Most presentation RON is already authored data, but embedding it still makes a save rebuild Rust.  
Runtime assets are loaded without file watching; direct filesystem readers also have no reload path.  
Reloading the source alone will leave copied UI, material, registry and blueprint values stale.  
Simulation changes belong at spawn or scenario reset, with matching content identity where required.  
Start with lighting, settings-page appearance and sight appearance; **DERIVED target: about one second**, unmeasured.

This is the inventory for [issue 276](https://github.com/vikng-dev/overmatch/issues/276), informed by [the dev-loop research](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/dev-loop-speed/research.md:116). The issue has no comments. Research was read-only: no edits, builds, tests or git state changes. The excluded documents were not opened, and no damage-topic web research was performed. References to the game’s damage model below concern authored game-state data and its presentation.

All findings and classifications below are **DERIVED from source inspection**. No save-to-visible duration was **MEASURED**. Paths, source-line locators, versions and identifiers are labels rather than quantitative estimates.

**Classification and notation**

- **C — compiled in, for a stated reason:** retain compilation in the initial migration because the current reader participates in shared content identity, wire interpretation, shipped integrity or construction before asynchronous assets are available. The supporting code is quoted below.
- **E — external data, loaded at runtime:** either already external, or an embedded/code-authored input suitable for migration. Presentation can refresh live; simulation applies to new construction at spawn/reset.
- **P:** presentation, including local camera/input feel. **S:** simulation. **W:** wire interpretation. **F:** shared content fingerprint.

“Loaded once” means once per application, registry or retained asset handle as described—not reread for every instance. No inventory row is currently watched.

**Embedded authored files**

Paths in this table are repository-relative.

| Input | Reader | Controls / audience | Today; construction copies and classification |
|---|---|---|---|
| `assets/world/lighting.ron` | [world/look.rs:6](/Users/Yan/github/vikng-dev/overmatch/src/world/look.rs:6) | Sun direction, colour, illuminance, analytic sky and environment light. **P** | Compiled; `LazyLock` parse. **E/live.** Refresh existing sun components, rebuild the sky/environment products and rebake the terrain-shadow field when direction changes. |
| `assets/ui/settings.ron` | [settings/look.rs:12](/Users/Yan/github/vikng-dev/overmatch/src/settings/look.rs:12) | Defaults, quality presets/ranges and settings-page appearance. **P** | Compiled; `LazyLock`. **E/live for appearance; C/early for boot defaults under current construction.** Defaults enter `Settings` and initial window construction; page values enter existing UI nodes. Do not replace player choices when refreshing appearance. |
| `assets/ui/sight.ron` | [sight/look.rs:11](/Users/Yan/github/vikng-dev/overmatch/src/sight/look.rs:11) | Sight dots, labels, optic surround, camera blend, mouse sensitivity and cursor reach. **P/local input** | Compiled; `LazyLock`. **E/live.** Some values are read during updates; others are copied into nodes and `OpticMaskMaterial`. |
| `assets/ui/graticule.ron` | [sight/reticle/look.rs:9](/Users/Yan/github/vikng-dev/overmatch/src/sight/reticle/look.rs:9) | Range-scale extent, graduation spacing, line sizes, labels and colour. **P** | Compiled; parsed into `GraticuleLook` resource. **E/live.** Reconstruct graduation entities when spacing/extent changes; refresh existing styles. |
| `assets/ui/overlay.ron` | [overlay/look.rs:69](/Users/Yan/github/vikng-dev/overmatch/src/overlay/look.rs:69) | Lobby, connection scrims, leave button and shared action-button appearance. **P** | Compiled; function-local `LazyLock`. **E/live.** Refresh existing nodes, fonts, backgrounds and interaction colours. |
| `assets/ui/death_screen.ron` | [net/death_screen.rs:406](/Users/Yan/github/vikng-dev/overmatch/src/net/death_screen.rs:406) | Out-of-action screen appearance. **P** | Compiled; `OnceLock`. **E/live.** Rebuild or refresh the currently mounted screen. |
| `assets/ui/spawn_map.ron` | [net/spawn_map.rs:97](/Users/Yan/github/vikng-dev/overmatch/src/net/spawn_map.rs:97) | Map-panel appearance **P**, plus `limit_fraction`, the shared spawn-coordinate bound **S** | Compiled; `OnceLock`. **E/live for style; E/reset for spawn policy.** `spawn_limit` reads the same style object on the authority. |
| `assets/input/drive_hud.ron` | [drive_hud.rs:42](/Users/Yan/github/vikng-dev/overmatch/src/drive_hud.rs:42) | Driving HUD fonts, placement, colours and gauge dimensions. **P** | Compiled; `LazyLock`. **E/live.** Existing UI components contain construction copies. |
| `assets/vfx/presentation.ron` | [vfx/look.rs:10](/Users/Yan/github/vikng-dev/overmatch/src/vfx/look.rs:10) | Game’s damage-model view: impact, muzzle-renderer, trail, mark, diagram and building-crack appearance. **P** | Compiled; `OnceLock`. **E/live.** Refresh cached materials/uniforms; new effects can use new parameters, while existing effects need an explicit refresh or retirement policy. |
| `assets/vfx/muzzle.ron` | [guns/muzzle.rs:25](/Users/Yan/github/vikng-dev/overmatch/src/guns/muzzle.rs:25) | Game’s muzzle-view layers and energy-scaled emission tastes. **P** | Compiled; `LazyLock`, then resolved into gun/round records. **C/early under current registry construction.** A presentation migration must refresh those derived records or move emission resolution to the view owner. |
| `assets/vfx/hull_marks.ron` | [hull_marks.rs:67](/Users/Yan/github/vikng-dev/overmatch/src/hull_marks.rs:67) | Game’s surface-mark grazing stretch; also footprint-based retained-mark selection. **P+W** | Compiled; cached scalar. **C/shared wire interpretation.** Both authority and replica apply the same mark-retention algorithm; this is not independently reloadable client style today. |
| `assets/terrain/track_prints.ron` | [track_prints.rs:93](/Users/Yan/github/vikng-dev/overmatch/src/track_prints.rs:93) | Cosmetic print contact, range, fade and sampling bounds. **P** | Compiled; `LazyLock`. **E/live.** `TrackPrints::new` copies values into `PrintLaw`; material uniforms also need refreshing. |
| `assets/driving/wheel_view.ron` | [track/wheels.rs:58](/Users/Yan/github/vikng-dev/overmatch/src/track/wheels.rs:58) | Wheel-view response parameter. **P** | Compiled; `LazyLock`. **E/live.** Refresh the view’s parameter reader; preserve or reset existing filter state deliberately. |
| `assets/shell/shell.model.ron` | [rounds/effects.rs:137](/Users/Yan/github/vikng-dev/overmatch/src/rounds/effects.rs:137) | Game’s round-body model dimensions. **P** | Compiled; `BodyModel` registry parsed once. **E/live.** Refresh cached model facts and existing scaled visuals where applicable. |
| `assets/input/drive.ron` | [track/drive.rs:97](/Users/Yan/github/vikng-dev/overmatch/src/track/drive.rs:97) | Tap/hold boundary, notches, lever rise and steering detents. **S/local input** | Compiled; `LazyLock`, read during input/simulation shaping. **E/reset.** Snapshot the policy for a scenario rather than changing running lever behavior mid-step. |
| `assets/driving/engine.ron` | [track/transmission.rs:78](/Users/Yan/github/vikng-dev/overmatch/src/track/transmission.rs:78) | Engine-control bands, drag and response. **S** | Compiled; `LazyLock`; used in derivation and running laws. **E/spawn or reset.** New construction must retain its admitted policy snapshot. |
| `assets/driving/actuation.ron` | [track/transmission.rs:109](/Users/Yan/github/vikng-dev/overmatch/src/track/transmission.rs:109) | Driver actuation and reversal/neutral policy. **S** | Compiled; `LazyLock`. **E/spawn or reset.** Includes values converted into countdown/state fields. |
| `assets/driving/grip.ron` | [track/forces.rs:50](/Users/Yan/github/vikng-dev/overmatch/src/track/forces.rs:50) | Generic grip/rolling-law parameters. **S** | Compiled; `LazyLock`; consulted by constructed grip systems. **E/spawn or reset.** Do not make existing entities silently read a newly replaced global law. |
| `assets/driving/driver.ron` | [track/load.rs:35](/Users/Yan/github/vikng-dev/overmatch/src/track/load.rs:35) | Driver observation/roundoff resolution. **S** | Compiled; `LazyLock`, copied by `DriverResolution::load`. **E/reset**, although these are authored `Resolution` values rather than art/balance migration priorities. |
| `assets/damage/crew.ron` | [damage.rs:149](/Users/Yan/github/vikng-dev/overmatch/src/damage.rs:149) | Game’s crew-station foreign-competence rate. **S** | Compiled; `LazyLock`, read by competence calculations. **E/spawn or reset.** Bind the rate to the new scenario/construction snapshot. |
| `assets/damage/repair.ron` | [damage/repair.rs:30](/Users/Yan/github/vikng-dev/overmatch/src/damage/repair.rs:30) | Game’s module-repair work rate. **S** | Compiled; `LazyLock`. **E/spawn or reset.** Existing repair progress must keep a coherent policy. |
| `assets/ballistics/physical_laws.ron` | [physical_laws.rs:7](/Users/Yan/github/vikng-dev/overmatch/src/physical_laws.rs:7) | Game’s physical-law facts and fitted tables. **S**, some client-derived presentation | Compiled; `LazyLock`; eagerly admitted during plugin construction. **C/early today.** A later external reader must load synchronously before construction, then apply only at reset. |
| `assets/ballistics/energy_dose.ron` | [ballistics/energy_dose.rs:14](/Users/Yan/github/vikng-dev/overmatch/src/ballistics/energy_dose.rs:14) | Game’s energy-to-HP conversion rate. **S** | Compiled; cached scalar, eagerly admitted while building the app. **C/early today.** |
| `assets/ballistics/box_classes.ron` | [ballistics/box_classes.rs:17](/Users/Yan/github/vikng-dev/overmatch/src/ballistics/box_classes.rs:17) | Game’s shared crew HP pool and derived view thresholds. **S+P** | Compiled; `LazyLock`; tank-spec parsing copies the pool into admitted construction. **C/early today.** |
| `assets/ballistics/spall_cone.ron` | [ballistics/spall_cone.rs:63](/Users/Yan/github/vikng-dev/overmatch/src/ballistics/spall_cone.rs:63) | Game’s field shapes, units and burst definitions. **S+F** | Compiled; multiple cached parsed products. **C/content identity and early admission.** Shapes are cloned into `ShotVocabulary`; replacing the parser cache alone is insufficient. |
| `assets/ballistics/round_types.ron` | [rounds.rs:90](/Users/Yan/github/vikng-dev/overmatch/src/rounds.rs:90) | Game’s round-type registry. **S+F** | Compiled; parsed with `RoundRegistry`. **C/content identity.** |
| `assets/rounds/{7.9mm_AP,88mm_AP,88mm_APCR,88mm_APHE_ring,88mm_HE}.round.ron` | [build.rs:16](/Users/Yan/github/vikng-dev/overmatch/build.rs:16), [rounds.rs:681](/Users/Yan/github/vikng-dev/overmatch/src/rounds.rs:681) | Game’s round facts, mechanisms and associated presentation declarations. **S+P+F** | Generated embedded index; `RoundRegistry` is a `LazyLock`. **C/content identity.** Already-constructed weapon/round records retain resolved values. |
| `assets/guns/{kwk36_l56,mg34}.gun.ron` | [build.rs:17](/Users/Yan/github/vikng-dev/overmatch/build.rs:17), [guns.rs:340](/Users/Yan/github/vikng-dev/overmatch/src/guns.rs:340) | Gun construction, ammunition binding, report clips and muzzle selection. **S+P+F** | Generated embedded index; `GunRegistry` is a `LazyLock`. **C/content identity.** Weapon records and warmed sound dependencies are downstream copies. |
| `assets/materials/materials.ron` | [substances.rs:90](/Users/Yan/github/vikng-dev/overmatch/src/substances.rs:90), [substances.rs:946](/Users/Yan/github/vikng-dev/overmatch/src/substances.rs:946) | Shared substance registry. **S+F**, visual-key references | Compiled; static cached registry plus separately constructed registries. **C/content identity.** Material-derived construction must be rebuilt coherently. |
| `assets/materials/attachments.ron` | [buildings/attachments.rs:29](/Users/Yan/github/vikng-dev/overmatch/src/buildings/attachments.rs:29) | Generic construction-joint definitions. **S** | Compiled; resolved once against substances. **E/reset.** Existing construction owns resolved joint facts. |
| `assets/tiger_1/tiger_1.tank.ron` | [bake.rs:400](/Users/Yan/github/vikng-dev/overmatch/src/bake.rs:400), [bake.rs:883](/Users/Yan/github/vikng-dev/overmatch/src/bake.rs:883) | Selected tank construction, stations, weapons, rig and view/audio references. **S+P+F** | Compiled; selected-spec cache and startup `TankBlueprint`. **C/content identity and early admission.** Spawn copies construction into components. The separate async reader is listed below. |
| `assets/input/aim.ron` | [aim/data.rs:6](/Users/Yan/github/vikng-dev/overmatch/src/aim/data.rs:6) | Aim horizon/admission envelope. **S/input+F** | Compiled; scalar `MAX_RANGE` cached once. **C/content identity.** |
| `assets/modes/battle.ron` | [net/battle_mode.rs:36](/Users/Yan/github/vikng-dev/overmatch/src/net/battle_mode.rs:36) | Battle duration, admission, respawn wait and spawn distances. **S** | Compiled; parsed when constructing the server. **E/next Battle reset.** Values are copied into cycle rules, `RespawnWait` and `SpawnPlacement` at [server.rs:197](/Users/Yan/github/vikng-dev/overmatch/src/net/server.rs:197). |
| `assets/net/client.ron` | [net/client/policy.rs:19](/Users/Yan/github/vikng-dev/overmatch/src/net/client/policy.rs:19) | Reconnect pacing and joining-screen hints. Client/network policy | Compiled; `LazyLock`. **E/next connection or retry sequence.** Preserve already-established request deadlines. |
| `build/icons/window_icon.png` | [branding.rs:17](/Users/Yan/github/vikng-dev/overmatch/src/branding.rs:17) | Window icon. **P** | Compiled bytes; decoded/applied at startup. **E** if branding reload is desired. Current embedding has a self-contained packaging rationale, rather than a simulation/wire requirement. |
| `assets/dev/frame_cost.ron` | [frame_cost.rs:134](/Users/Yan/github/vikng-dev/overmatch/src/frame_cost.rs:134) | Capture window parking. Dev presentation | Compiled; `LazyLock`. **E/next capture/window placement.** |
| `assets/dev/track_sandbox.ron` | [track_sandbox/data.rs:7](/Users/Yan/github/vikng-dev/overmatch/src/track_sandbox/data.rs:7) | Sandbox geometry, camera, lighting and visualization styles. **S+P/dev** | Compiled; `OnceLock`. **E/reset for geometry, live for styles.** Constructed colliders/meshes are copies. |
| `assets/dev/wall-tool.ron` | [sandbox/wall.rs:385](/Users/Yan/github/vikng-dev/overmatch/src/sandbox/wall.rs:385) | Game’s wall-tool inspector and view configuration. **P/dev** | Compiled; `LazyLock`. **E/live**, refreshing constructed inspector/material values. |
| `assets/dev/capture-wall.ron` | [dev_capture/wall.rs:128](/Users/Yan/github/vikng-dev/overmatch/src/dev_capture/wall.rs:128) | Capture staging and framing. **S+P/dev** | Compiled; cached parse. **E/next capture/reset.** |
| Additional sandbox embeddings of the tank sheet and `assets/buildings/house_1/house_1.building.ron` | [sandbox.rs:77](/Users/Yan/github/vikng-dev/overmatch/src/sandbox.rs:77), [sandbox/wall.rs:63](/Users/Yan/github/vikng-dev/overmatch/src/sandbox/wall.rs:63) | Range gun selection and wall construction. **S+P/dev** | Compiled; parsed into sandbox construction. **E/reset for sandbox use.** These duplicate files already inventoried as production inputs. |

**Generated Rust and the reasons for compile-bound classification**

The build script produces only `$OUT_DIR/armaments_index.rs`, included by [lib.rs:18](/Users/Yan/github/vikng-dev/overmatch/src/lib.rs:18). It enumerates gun and round files in sorted order and emits embedded source text:

```rust
writeln!(text, "    ({stem:?}, include_str!({path:?})),").unwrap();
```

The output path and write are explicit at [build.rs:38](/Users/Yan/github/vikng-dev/overmatch/build.rs:38):

```rust
let target = out.join("armaments_index.rs");
std::fs::write(&target, text)
```

Directory/file changes trigger Cargo reruns. The filename/suffix refusal in [build.rs:57](/Users/Yan/github/vikng-dev/overmatch/build.rs:57) is also a shipping-integrity gate; it can remain independently of a future runtime reader.

The connect fingerprint’s actual inputs—not its prose description—are visible at [net/protocol.rs:310](/Users/Yan/github/vikng-dev/overmatch/src/net/protocol.rs:310):

```rust
handshake_tag(
    crate::map::content_digest(&root),
    crate::substances::MATERIALS_RON.as_bytes(),
    crate::bake::construction_digest(&root),
    armaments_digest(
        crate::rounds::ROUND_TYPES_RON,
        crate::armaments_index::ROUND_FILES,
        crate::armaments_index::GUN_FILES,
    ),
    crate::aim::INPUT_RON.as_bytes(),
)
```

Construction fingerprints the embedded sheet alongside the runtime sim artifact at [bake.rs:427](/Users/Yan/github/vikng-dev/overmatch/src/bake.rs:427):

```rust
[TANK_SPEC_RON.as_bytes(), sim.as_slice()].iter().fold(...)
```

The game’s explanation vocabulary has another content fingerprint. [damage_cam/delivery.rs:61](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/delivery.rs:61) constructs it from the blueprint, round registry, cone shape and burst fields. [damage_cam/wire.rs:436](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/wire.rs:436) folds the shapes; decoding refuses a mismatch at [wire.rs:1273](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/wire.rs:1273):

```rust
if fingerprint != vocabulary.fingerprint {
    return Err(...);
}
```

Early admission is also real. [ballistics.rs:1940](/Users/Yan/github/vikng-dev/overmatch/src/ballistics.rs:1940) calls these readers while constructing the plugin:

```rust
spall_cone::ConeShape::shipped();
spall_cone::cone_unit();
spall_cone::BurstFields::shipped();
box_classes::BoxClasses::shipped();
energy_dose::joules_per_hp();
physical_laws::PhysicalLaws::shipped();
```

Similarly, [settings.rs:1667](/Users/Yan/github/vikng-dev/overmatch/src/settings.rs:1667) reads settings before constructing the initial window, and [sfx/mod.rs:106](/Users/Yan/github/vikng-dev/overmatch/src/sfx/mod.rs:106) reads `selected_spec()` in `PreStartup`, before the startup blueprint exists. Muzzle presentation is resolved synchronously with the gun registry at [guns.rs:234](/Users/Yan/github/vikng-dev/overmatch/src/guns.rs:234).

**Decision:** preserve these early construction requirements during the first presentation migration. They require synchronously admitted data, not embedding forever. A later synchronous runtime preload could replace the embedding without waiting for asynchronous view assets.

The game’s hull-mark data has an additional shared-behavior constraint. [hull_marks.rs:305](/Users/Yan/github/vikng-dev/overmatch/src/hull_marks.rs:305) selects retained marks using:

```rust
mark.caliber().powi(2) * stretch(mark.along(), mark.normal())
```

The replica applies that same algorithm at [net/hull_marks.rs:20](/Users/Yan/github/vikng-dev/overmatch/src/net/hull_marks.rs:20):

```rust
fn apply_diff(&mut self, mark: &HullMark) -> Result<()> {
    self.record(*mark);
    Ok(())
}
```

Changing the stretch taste independently can therefore change retained replicated content. Keep it outside the first client-only reload slice.

**Runtime-loaded data and assets**

Every row below is **E — external data, loaded at runtime**. Fingerprinting and integrity checks constrain replacement; they do not make these files embedded.

| Input | Reader | Controls / audience | Today; copied state requiring refresh |
|---|---|---|---|
| `assets/maps/kalinovo/level.json` | [map.rs:464](/Users/Yan/github/vikng-dev/overmatch/src/map.rs:464) | Map extent, decoding conventions, placements and spawn declarations. **S+P+F** | Loaded at startup into `MapManifest`; terrain/scatter/spawn construction copies it. Apply at scenario reset. |
| `assets/maps/kalinovo/prototypes.ron` | [map.rs:455](/Users/Yan/github/vikng-dev/overmatch/src/map.rs:455) | Placement IDs, geometry, substances and referenced construction. **S+P+F** | Loaded at map startup; retained in manifest/library, then spawned. Rebuild scenario. |
| Manifest-selected heightmap, currently `assets/maps/kalinovo/terrain_height.png` | [terrain_grid.rs:570](/Users/Yan/github/vikng-dev/overmatch/src/terrain_grid.rs:570) | Terrain samples, collider, driving oracle and rendered mesh. **S+P+F** | Decoded once into `HeightGrid`; derived geometry must be rebuilt at reset. |
| Manifest-selected masks, currently `assets/maps/kalinovo/terrain_masks.png` | [ground_field.rs:197](/Users/Yan/github/vikng-dev/overmatch/src/ground_field.rs:197) | Painted ground weights for driving and terrain appearance. **S+P** | Decoded at startup into `GroundField`; renderer receives derived image bytes. Reset both sides together. **No current connect-fingerprint term found.** |
| `assets/terrain/ground_types.ron` | [ground_types.rs:210](/Users/Yan/github/vikng-dev/overmatch/src/ground_types.rs:210) | Ground grip, rolling/contact response, dust and print response. **S+P** | Reread at construction callsites, not watched. Values enter `GroundField`, collider friction and view resources/materials. **No current connect-fingerprint term found.** |
| `assets/trees/species.ron`, `wood.ron`, `<id>/<id>.tree.ron`; selected tree currently `fir_1` | [trees.rs:410](/Users/Yan/github/vikng-dev/overmatch/src/trees.rs:410) | Tree generation, material and construction facts. **S+P+F** | Loaded into `TreeLibrary` and `Arc<TreeKind>` at map startup; apply through reconstructed trees at reset. |
| Map-selected `assets/buildings/<id>/<id>.building.ron`; shipped definitions include `house_1` and `fir_1` | [buildings.rs:194](/Users/Yan/github/vikng-dev/overmatch/src/buildings.rs:194) | Building construction and admitted scales. **S+P+F** | Loaded into `BuildingLibrary`/resolved construction; rebuild placed structures at reset. |
| `assets/tiger_1/tiger_1.tank.ron`, async path | [spec.rs:1981](/Users/Yan/github/vikng-dev/overmatch/src/spec.rs:1981); requested at [tank/spawn.rs:209](/Users/Yan/github/vikng-dev/overmatch/src/tank/spawn.rs:209) | Async spec used by view binding and viewers. **P**, duplicating production construction data | Requested at startup/view load and retained. Reloading this asset does **not** replace the embedded source used by `TankBlueprint`. |
| `assets/tiger_1/tiger_1.sim.glb` | [bake.rs:881](/Users/Yan/github/vikng-dev/overmatch/src/bake.rs:881), [track/marker_model.rs:235](/Users/Yan/github/vikng-dev/overmatch/src/track/marker_model.rs:235) | Synchronous spawn geometry, markers and collision/game-state construction. **S+F** | Read at startup. Geometry enters `TankBlueprint`, `TrackColliders` and derived rig data. New construction only. |
| `assets/tiger_1/tiger_1.lod.json` | [geometry_lod/certificate.rs:119](/Users/Yan/github/vikng-dev/overmatch/src/geometry_lod/certificate.rs:119) | Artifact hashes and certified LOD measurements. **P/integrity** | Loaded once into `TankCertificate`; matching artifacts and derived chains/bands must be admitted together. |
| `assets/tiger_1/tiger_1.glb`, including contained material/image data | [tank/spawn.rs:210](/Users/Yan/github/vikng-dev/overmatch/src/tank/spawn.rs:210), [geometry_lod.rs:202](/Users/Yan/github/vikng-dev/overmatch/src/geometry_lod.rs:202) | Tank view and LOD meshes. **P/integrity** | Loaded once, instantiated per tank. Cached mesh/material/binding products require refresh. Shoe-template admission also reads its document at [track/link_view.rs:588](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:588). |
| `assets/trees/<id>/<id>.glb`, currently `fir_1` | [trees/view.rs:88](/Users/Yan/github/vikng-dev/overmatch/src/trees/view.rs:88) | Tree view. **P** | Loaded per kind, extracted into cached `TreeTemplates` and merged standing meshes. Invalidate/rebuild those products. |
| `assets/shell/shell.glb` and future scenes named by body-model data | [ballistics/view.rs:77](/Users/Yan/github/vikng-dev/overmatch/src/ballistics/view.rs:77), [vfx/prewarm.rs:68](/Users/Yan/github/vikng-dev/overmatch/src/vfx/prewarm.rs:68) | Game’s round-body view. **P** | Preloaded into `ProjectileAssets`, instantiated per shot; viewers also cache scenes/materials. |
| `assets/materials/visuals.ron` | [tank_surface.rs:364](/Users/Yan/github/vikng-dev/overmatch/src/tank_surface.rs:364), [building_surface.rs:97](/Users/Yan/github/vikng-dev/overmatch/src/building_surface.rs:97) | Shared visual packs, colours, surface parameters, repeats and building lanes. **P** | Tank `SurfaceData` constructed once; building palette uses a process-global `LazyLock`. Refresh cached looks/coats and existing material parameters. |
| `assets/tiger_1/tiger_1.livery.ron` | [tank_surface.rs:371](/Users/Yan/github/vikng-dev/overmatch/src/tank_surface.rs:371) | Paint and camouflage. **P** | Loaded with surface data; copied into cached coats and material parameters. |
| `assets/materials/visuals/{rolled_steel,cast_steel,machined_steel,track_steel,rubber}/{same}_{diff,nor_gl,arm}.ktx2` | [tank_surface.rs:891](/Users/Yan/github/vikng-dev/overmatch/src/tank_surface.rs:891) | Tank surface maps. **P** | Loaded on first use; handles cached by visual. Some painted visuals omit the albedo map. Refresh bindings when replacement changes their required resources. |
| `assets/buildings/{brick_wall_006,dark_planks}/{same}_{diff,nor_gl,arm}.ktx2` | [building_surface.rs:210](/Users/Yan/github/vikng-dev/overmatch/src/building_surface.rs:210) | Building surface maps. **P** | Loaded during shared material construction. Paths, repeats and UV transforms have cached copies. |
| `assets/materials/camouflage/<pattern>.ktx2`, currently `stripes.ktx2` | [tank_surface.rs:945](/Users/Yan/github/vikng-dev/overmatch/src/tank_surface.rs:945) | Camouflage coverage. **P** | Loaded when the coat is first created and cached with it. |
| `assets/terrain/coast_sand_rocks_02/coast_sand_rocks_02_{diff,nor_gl,arm}.ktx2` | [world.rs:646](/Users/Yan/github/vikng-dev/overmatch/src/world.rs:646) | Base terrain appearance. **P** | Loaded at environment construction into material handles and `TerrainMaps`. |
| `assets/terrain/blend/blend_{diff,nor_gl,arm}.ktx2` | [world.rs:666](/Users/Yan/github/vikng-dev/overmatch/src/world.rs:666) | Packed layered terrain appearance. **P** | Loaded at environment construction; array layout must agree with the consumer. |
| `assets/vfx/{flash_core_atlas.png,blast_core_a.ktx2,blast_core_b.ktx2,mg_core.png,flash_flames_atlas.png,smoke_atlas.png,impact_dust.png,spark_atlas.png}` | Path list [vfx/mod.rs:139](/Users/Yan/github/vikng-dev/overmatch/src/vfx/mod.rs:139); loader [vfx/mod.rs:160](/Users/Yan/github/vikng-dev/overmatch/src/vfx/mod.rs:160) | Effect textures. **P** | Preloaded; handles copied into material resources. Atlas metadata and image layout must remain coherent. |
| `assets/fonts/BarlowCondensed-{SemiBold,Regular}.ttf` | [ui_font.rs:117](/Users/Yan/github/vikng-dev/overmatch/src/ui_font.rs:117) | Game UI typography. **P** | Loaded into `UiFonts`; handles copied into text components. Changed paths require component refresh; changed coverage still needs the existing glyph gate. |
| `assets/maps/kalinovo/derived/map_ui.png` | [net/spawn_map.rs:464](/Users/Yan/github/vikng-dev/overmatch/src/net/spawn_map.rs:464) | Spawn-map backdrop. **P** | Loaded during overlay construction and retained in UI. This is a derived artifact. |
| `assets/sfx/click/click_1.ogg`; sheet-selected engine clip, currently `sfx/engine/engine_loop_44hz.ogg`; gun-selected reports, currently `sfx/explosion/88_cannon_{1..5}.ogg` | [sfx/mod.rs:103](/Users/Yan/github/vikng-dev/overmatch/src/sfx/mod.rs:103), [sfx/mod.rs:143](/Users/Yan/github/vikng-dev/overmatch/src/sfx/mod.rs:143) | Audio. **P** | Loaded/prewarmed and cached by path. Paths/settings are also copied into `Weapon`, `EngineSound` and playback construction. Active playback needs explicit refresh/restart handling. |

The runtime certificate gate proves that integrity does not require embedding. [geometry_lod/certificate.rs:207](/Users/Yan/github/vikng-dev/overmatch/src/geometry_lod/certificate.rs:207) checks:

```rust
assert!(measured == recorded, ...);
```

Likewise, the current map fingerprint includes manifest text, prototype text, heightmap bytes and selected tree/building texts at [map.rs:503](/Users/Yan/github/vikng-dev/overmatch/src/map.rs:503). It does not include terrain-mask bytes or the ground table. Those omissions matter when defining a future coherent scenario-data snapshot.

**Runtime shaders**

All paths below are under `assets/shaders/`. All are **E/P**, loaded on material/pipeline use and retained, currently without watching. Reloading shader source and refreshing CPU-authored uniforms are separate jobs.

| Shader | Reader |
|---|---|
| `terrain_blend.wgsl` | [terrain_blend.rs:322](/Users/Yan/github/vikng-dev/overmatch/src/terrain_blend.rs:322) |
| `tank_surface.wgsl` | [tank_surface.rs:440](/Users/Yan/github/vikng-dev/overmatch/src/tank_surface.rs:440) |
| `building_surface.wgsl` | [building_surface.rs:155](/Users/Yan/github/vikng-dev/overmatch/src/building_surface.rs:155) |
| `building_surface_prepass.wgsl` | [building_surface.rs:163](/Users/Yan/github/vikng-dev/overmatch/src/building_surface.rs:163) |
| `building_holes.wgsl` | Imported by [building_surface.wgsl:33](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/building_surface.wgsl:33) and [building_surface_prepass.wgsl:10](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/building_surface_prepass.wgsl:10) |
| `render_scale_upscale.wgsl` | [render_scale.rs:319](/Users/Yan/github/vikng-dev/overmatch/src/render_scale.rs:319) |
| `optic_mask.wgsl` | [sight/reticle.rs:511](/Users/Yan/github/vikng-dev/overmatch/src/sight/reticle.rs:511) |
| `track_belt_mesh.wgsl` | [track/belt_draw.rs:135](/Users/Yan/github/vikng-dev/overmatch/src/track/belt_draw.rs:135) |
| `track_belt.wgsl` | Imported by [track_belt_mesh.wgsl:9](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/track_belt_mesh.wgsl:9) and [pen_cam_look_belt.wgsl:7](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/pen_cam_look_belt.wgsl:7) |
| `vfx_billboard.wgsl` | [vfx/billboard.rs:131](/Users/Yan/github/vikng-dev/overmatch/src/vfx/billboard.rs:131) |
| `vfx_trail.wgsl` | [vfx/trail.rs:123](/Users/Yan/github/vikng-dev/overmatch/src/vfx/trail.rs:123) |
| `pen_cam_look.wgsl` | [damage_cam/pen_cam/look.rs:335](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/look.rs:335) |
| `pen_cam_look_belt.wgsl` | [damage_cam/pen_cam/look.rs:413](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/look.rs:413) |
| `pen_cam_glow.wgsl` | [damage_cam/pen_cam/material.rs:57](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/material.rs:57) |
| `pen_cam_outline.wgsl` | [damage_cam/pen_cam/material.rs:134](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/material.rs:134) |
| `pen_cam_slice.wgsl` | [damage_cam/pen_cam/material.rs:193](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/material.rs:193) |
| `pen_cam_picture.wgsl` | [damage_cam/pen_cam/picture.rs:1017](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/picture.rs:1017) |

**Code-authored values that belong in data**

These are samples per module, not an enumeration of literals. Each is compiled today. Unless stated otherwise, the classification is **E**: presentation refreshes live; simulation is snapshotted for the next spawn/reset. Samples name constants without treating their present values as obligations.

| Module / sample reader | Controls / audience | Reload boundary or copied state |
|---|---|---|
| [camera.rs:171](/Users/Yan/github/vikng-dev/overmatch/src/camera.rs:171): `PIVOT_LIFT`, orbit limits; [camera.rs:376](/Users/Yan/github/vikng-dev/overmatch/src/camera.rs:376): sensitivity and zoom easing | Camera geometry and input feel. **P** | Mostly per-frame readers; refresh defaults and any initialized camera state. |
| [firecontrol.rs:78](/Users/Yan/github/vikng-dev/overmatch/src/firecontrol.rs:78): range envelope; [firecontrol.rs:209](/Users/Yan/github/vikng-dev/overmatch/src/firecontrol.rs:209): range-dial choices | Aiming range policy. **P/input** | Rebuild cached range tables when the envelope changes; refresh dial bounds. Numerical integration/search steps remain resolution. |
| [ui_font.rs:27](/Users/Yan/github/vikng-dev/overmatch/src/ui_font.rs:27): overlay font size, text/panel colours | Shared UI style. **P** | Existing text/background components contain copies. |
| [hud.rs:105](/Users/Yan/github/vikng-dev/overmatch/src/hud.rs:105): modal gap; [hud.rs:313](/Users/Yan/github/vikng-dev/overmatch/src/hud.rs:313): inline label style | HUD layout and appearance. **P** | Refresh nodes/fonts/materials; layout gap is also read during layout. |
| [crew_ui.rs:66](/Users/Yan/github/vikng-dev/overmatch/src/crew_ui.rs:66): inline font, colour and offsets | Game’s crew-status UI. **P** | Refresh existing nodes. |
| [overlay.rs:68](/Users/Yan/github/vikng-dev/overmatch/src/overlay.rs:68): overlay stack positions | Overlay ordering. **P** | Refresh `GlobalZIndex`; validate relative ordering if exposed as authored style. |
| [settings.rs:249](/Users/Yan/github/vikng-dev/overmatch/src/settings.rs:249), [settings.rs:444](/Users/Yan/github/vikng-dev/overmatch/src/settings.rs:444): quality-rung magnitudes | Offered graphics choices. **P** | Reconcile applied resources; enum/schema changes remain code changes. |
| [settings/displays.rs:36](/Users/Yan/github/vikng-dev/overmatch/src/settings/displays.rs:36): label truncation | Display-name presentation. **P** | Regenerate offered labels. |
| [net/battle_hud.rs:38](/Users/Yan/github/vikng-dev/overmatch/src/net/battle_hud.rs:38): row size | Scoreboard density. **P** | Refresh line heights and recompute row capacity. |
| [net/debug_hud.rs:43](/Users/Yan/github/vikng-dev/overmatch/src/net/debug_hud.rs:43): refresh interval/warning threshold | Diagnostics presentation policy. **P** | Rearm refresh schedule and recolour existing labels. |
| [net/hit_feel.rs:22](/Users/Yan/github/vikng-dev/overmatch/src/net/hit_feel.rs:22): cue decay; inline cue styling | Feedback fade and appearance. **P** | Per-frame decay readers plus construction copies in cue UI. |
| [net/death_screen.rs:125](/Users/Yan/github/vikng-dev/overmatch/src/net/death_screen.rs:125): respawn request timeout | Client-flow policy | Apply to the next request; existing request holds a deadline. |
| [sfx/mod.rs:31](/Users/Yan/github/vikng-dev/overmatch/src/sfx/mod.rs:31): spatial scale, listener gap, volumes, smoothing | Audio mix and playback feel. **P** | Refresh `DefaultSpatialScale`, listener and playback settings; smoothing is read during updates. |
| [terrain_grid.rs:107](/Users/Yan/github/vikng-dev/overmatch/src/terrain_grid.rs:107): texture scale | Physical surface-map scale. **P** | Existing mesh UVs contain copies; rebuild UVs or move scale to uniforms. |
| [terrain_blend.rs:90](/Users/Yan/github/vikng-dev/overmatch/src/terrain_blend.rs:90), [terrain_blend.rs:192](/Users/Yan/github/vikng-dev/overmatch/src/terrain_blend.rs:192): layer definitions, AO bias, blend window, macro variation | Terrain appearance. **P** | Refresh materials and derived cosmetic responses. Layer order must agree with packed texture/mask interpretation. |
| [terrain_lod.rs:101](/Users/Yan/github/vikng-dev/overmatch/src/terrain_lod.rs:101): picture error budget | Terrain LOD quality choice. **P** | Recompute visibility ranges. Mesh-extraction resolution schedules are separate. |
| [terrain_shadow.rs:74](/Users/Yan/github/vikng-dev/overmatch/src/terrain_shadow.rs:74): sun-disc fact and softness | Terrain shadows. **P** | Refresh `HeightfieldShadow` parameters; direction changes require field rebake. |
| [tank/shadow_proxy.rs:46](/Users/Yan/github/vikng-dev/overmatch/src/tank/shadow_proxy.rs:46): switch range/hysteresis | Shadow-detail transition. **P** | Refresh the reconciliation policy and existing caster scopes. |
| [trees/view.rs:393](/Users/Yan/github/vikng-dev/overmatch/src/trees/view.rs:393): tree view range | Tree visibility. **P** | Refresh installed visibility ranges. |
| [track/view.rs:89](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:89): phase-heal smoothing | Drawn-track recovery feel. **P** | Per-frame reader; choose whether filter state survives reload. Other numerical guards require individual Resolution justification. |
| [track_prints/stamper.rs:45](/Users/Yan/github/vikng-dev/overmatch/src/track_prints/stamper.rs:45): resting-facing/sink admission | Cosmetic print admission. **P** | New stamps use new policy; existing printed history remains. |
| [track_prints/store.rs:58](/Users/Yan/github/vikng-dev/overmatch/src/track_prints/store.rs:58): near-store world coverage; [store.rs:85](/Users/Yan/github/vikng-dev/overmatch/src/track_prints/store.rs:85): pressure encoding unit | Cosmetic store coverage and CPU/shader representation. **P** | Reconstruct stores for coverage changes; encoding changes require coherent CPU/shader refresh and re-encoding. Texel/tile evaluation sizes are Resolution candidates. |
| [track_prints/tuning.rs:26](/Users/Yan/github/vikng-dev/overmatch/src/track_prints/tuning.rs:26): panel width | Dev UI. **P** | Refresh layout. |
| [damage_cam/pen_cam/hud.rs:28](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/hud.rs:28): inset width fraction | Game’s explanation-view size. **P** | Recompute viewport, furniture and size-dependent picture products. |
| [damage_cam/pen_cam/look.rs:89](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/look.rs:89): state opacity/palette | Game’s explanation-view appearance. **P** | Refresh existing materials/uniforms. |
| [damage_cam/pen_cam/stage.rs:64](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/stage.rs:64): path glow and framing tastes | Game’s explanation stage. **P** | Update live readers; regenerate stage meshes/materials containing copies. |
| [damage_cam/pen_cam/timeline.rs:89](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/timeline.rs:89): slowdown, beats and state colours | Game’s replay pacing/style. **P** | Rebuild the current presentation plan coherently or apply to the next replay. |
| [damage_cam/pen_cam/killcam.rs:43](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/killcam.rs:43): phase durations/bearings | Replay choreography. **P/client flow** | Next replay, or reconstruct its timing and camera together. |
| [damage_cam/pen_cam/game/killcam.rs:90](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/game/killcam.rs:90): record wait | Replay readiness policy | Next pending view/request. |
| [damage_cam/delivery.rs:41](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/delivery.rs:41): read window and impact timeout | Explanation retention/release. **P** | Refresh store/window policy coherently; existing arrival timestamps remain. |
| [damage_cam/pen_cam/material.rs:104](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/material.rs:104): outline sort bias | Transparent presentation ordering. **P** | Refresh affected materials while preserving ordering constraints. |
| [debug.rs:26](/Users/Yan/github/vikng-dev/overmatch/src/debug.rs:26): force-arrow scale; marker style | Debug presentation. **P** | Gizmos refresh per frame; cached marker mesh/material needs regeneration. |
| [vfx/billboard.rs:228](/Users/Yan/github/vikng-dev/overmatch/src/vfx/billboard.rs:228), [vfx/muzzle.rs:45](/Users/Yan/github/vikng-dev/overmatch/src/vfx/muzzle.rs:45): atlas layout metadata | Effect image interpretation. **P** | External asset metadata, validated against matching images; rebuild dependent material/effect families. |
| [shooting.rs:27](/Users/Yan/github/vikng-dev/overmatch/src/shooting.rs:27): `RECOIL_FEEL` | Applied hull impulse. **S**, despite its name | Snapshot for new construction; do not reinterpret existing replicated state. |
| [damage.rs:173](/Users/Yan/github/vikng-dev/overmatch/src/damage.rs:173): swap duration | Game’s crew-station swap policy. **S** | Copied into `PendingSwap.remaining` at [damage.rs:491](/Users/Yan/github/vikng-dev/overmatch/src/damage.rs:491). Existing countdowns remain coherent. |
| [track/derive.rs:23](/Users/Yan/github/vikng-dev/overmatch/src/track/derive.rs:23): gravity fact | Derived running-gear construction. **S** | Rebuild at spawn/reset; reconcile ownership with the authored physical-law table. |
| [terrain_grid.rs:143](/Users/Yan/github/vikng-dev/overmatch/src/terrain_grid.rs:143): spawn clearance/footprint | Spawn placement. **S** | Next spawn only. |
| [world.rs:100](/Users/Yan/github/vikng-dev/overmatch/src/world.rs:100), [world.rs:827](/Users/Yan/github/vikng-dev/overmatch/src/world.rs:827): slab/course geometry and styles | Flat-world/dev scenario. **S+P** | Reconstruct on scenario reset. |
| [net/server.rs:960](/Users/Yan/github/vikng-dev/overmatch/src/net/server.rs:960): bot spawn layout | Scenario placement. **S** | Next bot spawn/reset. Search-grid evaluation resolution is separate. |
| [net/sync_margin.rs:92](/Users/Yan/github/vikng-dev/overmatch/src/net/sync_margin.rs:92): starvation budget | Interpolation-delay product policy. **P/network timing** | Rebuild quantile/ring configuration coherently. |

Operational choices are also authored inputs, although they are poor first live-preview targets:

| Module / sample | Classification and application boundary |
|---|---|
| [net/matchmaking/ping.rs:20](/Users/Yan/github/vikng-dev/overmatch/src/net/matchmaking/ping.rs:20): sample count, spacing, timeout | **E**, next probe batch. Measurement-policy magnitudes need named ownership. |
| [net/matchmaking/ticket.rs:10](/Users/Yan/github/vikng-dev/overmatch/src/net/matchmaking/ticket.rs:10): polling/failure policy | **E**, next request sequence. |
| [net/matchmaking/worker.rs:22](/Users/Yan/github/vikng-dev/overmatch/src/net/matchmaking/worker.rs:22): HTTP timeout | **E**, next request/client construction. |
| [net/hosting.rs:37](/Users/Yan/github/vikng-dev/overmatch/src/net/hosting.rs:37): request/report timeouts | **E**, next request/report. |
| [net/client.rs:47](/Users/Yan/github/vikng-dev/overmatch/src/net/client.rs:47), [net/server.rs:44](/Users/Yan/github/vikng-dev/overmatch/src/net/server.rs:44): connection defaults | **E/operator configuration**, next connection/listener. |
| [settings/store.rs:44](/Users/Yan/github/vikng-dev/overmatch/src/settings/store.rs:44): rename retry budget | **E/policy**, snapshot per write; alternatively justify numerical retry bounds as Resolution. |
| [render_cost.rs:99](/Users/Yan/github/vikng-dev/overmatch/src/render_cost.rs:99): diagnostic sampling periods | **E**, next sampling schedule. |
| [frame_cost.rs:147](/Users/Yan/github/vikng-dev/overmatch/src/frame_cost.rs:147): capture surface/scale | **E**, reconstruct capture/window. |
| [quit.rs:327](/Users/Yan/github/vikng-dev/overmatch/src/quit.rs:327): latency acceptance budget | **E/certification policy**, next check. |
| [assets.rs:75](/Users/Yan/github/vikng-dev/overmatch/src/assets.rs:75): selected tank ID | **C/early selection today**: `tank_file!` expands the literal through `concat!` into embedded and runtime paths. Runtime selection needs an admitted content selection before construction. |
| [net/client.rs:52](/Users/Yan/github/vikng-dev/overmatch/src/net/client.rs:52): `option_env!("OVERMATCH_DEFAULT_API")` | Compiled deployment default; **E/operator configuration** on the next connection if externalized. |

Dev scenarios still hold substantial authored data in Rust:

| Module / sample | Controls / application boundary |
|---|---|
| [tank/scenario.rs:137](/Users/Yan/github/vikng-dev/overmatch/src/tank/scenario.rs:137): start spots, scripted aim/fire timing, duel/grid placement | **E/S+P**; reset scenario. |
| [tank/battle.rs:104](/Users/Yan/github/vikng-dev/overmatch/src/tank/battle.rs:104): layout, scripted firing and steering | **E/S+P**; reset pricing scenario. |
| [lod_showcase.rs:37](/Users/Yan/github/vikng-dev/overmatch/src/lod_showcase.rs:37): lane/staging layout | **E/S+P**; restage/reset. |
| [hull_marks/demo.rs:35](/Users/Yan/github/vikng-dev/overmatch/src/hull_marks/demo.rs:35): game’s mark-demo schedule/camera | **E/S+P**; restart demonstration. |
| [track_prints/demo.rs:49](/Users/Yan/github/vikng-dev/overmatch/src/track_prints/demo.rs:49): drive script/camera | **E/S+P**; restart demonstration. |
| [dev_capture.rs:63](/Users/Yan/github/vikng-dev/overmatch/src/dev_capture.rs:63), [dev_capture/fuze.rs:19](/Users/Yan/github/vikng-dev/overmatch/src/dev_capture/fuze.rs:19): warmup, staging, orchestration timeout | **E/dev policy**; next capture. |
| [net/debris_stress.rs:52](/Users/Yan/github/vikng-dev/overmatch/src/net/debris_stress.rs:52): stress-scene construction | **E/S**; reconstruct scenario. |
| [track_sandbox/rubble.rs:27](/Users/Yan/github/vikng-dev/overmatch/src/track_sandbox/rubble.rs:27): rubble layout | **E/S**; reconstruct scenario. |
| [track_sandbox/panel.rs:71](/Users/Yan/github/vikng-dev/overmatch/src/track_sandbox/panel.rs:71), [track_sandbox/suspension_viz.rs:133](/Users/Yan/github/vikng-dev/overmatch/src/track_sandbox/suspension_viz.rs:133) | **E/P**; next panel/gizmo draw. |
| [track_sandbox/harness.rs:57](/Users/Yan/github/vikng-dev/overmatch/src/track_sandbox/harness.rs:57): workload defaults | **E/dev policy**; next run. |
| [bitprobe.rs:42](/Users/Yan/github/vikng-dev/overmatch/src/bitprobe.rs:42): probe script/spawn pose | **E/dev scenario**; next probe run. |

Exact unit conversions, mathematical law-form constants, enum discriminants, type layouts and genuine evaluation Resolution are excluded from migration candidates. Examples include numerical solver tolerances, texture sampling/upload granularity and timing-limiter precision.

Wire definitions remain **C/W**. [net/protocol.rs:282](/Users/Yan/github/vikng-dev/overmatch/src/net/protocol.rs:282) makes the reason explicit:

```rust
pub const PROTOCOL_FINGERPRINT: u64 = protocol_fingerprint_for(
    WIRE_SURFACE_HASH,
    WIRE_TYPES_HASH,
    WIRE_DEP_AVIAN3D,
    WIRE_DEP_LIGHTYEAR,
    PROTOCOL_REV,
    env!("CARGO_PKG_VERSION"),
);
```

The encoding definitions in [net/wire_quant.rs:35](/Users/Yan/github/vikng-dev/overmatch/src/net/wire_quant.rs:35), including position/rotation/belt precision and layouts, belong to this contract. They are not live presentation sliders.

**Other runtime entry points and non-runtime files**

| Input | Reader | Classification / lifetime |
|---|---|---|
| Player `video.ron` in platform config storage, optionally relocated by `OVERMATCH_CONFIG_DIR` | [settings/store.rs:137](/Users/Yan/github/vikng-dev/overmatch/src/settings/store.rs:137), [store.rs:263](/Users/Yan/github/vikng-dev/overmatch/src/settings/store.rs:263) | **E/P**, loaded at boot. UI changes already flow through `Settings`; external file changes are not watched. |
| `OVERMATCH_CAPTURE=<plan>.ron`; examples under `scripts/capture/` | [dev_capture/plan.rs:442](/Users/Yan/github/vikng-dev/overmatch/src/dev_capture/plan.rs:442) | **E/S+P/dev**, parsed once per run. Includes camera/cue/staging declarations. |
| `OVERMATCH_HARNESS=<script>.ron`; shipped `scripts/harness/{idle,flat-patch}.ron` | [net/harness.rs:237](/Users/Yan/github/vikng-dev/overmatch/src/net/harness.rs:237) | **E/dev workload**, loaded once into a script resource. |
| Local game’s explanation/report RON, including `assets/dev/pen_cam/*.ron` when requested | [damage_cam/mod.rs:464](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/mod.rs:464), [damage_cam/validation.rs:201](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/validation.rs:201) | **E/viewer input**, loaded on request; may also read the named vehicle’s sheet/geometry through [damage_cam/shapes.rs:93](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/shapes.rs:93). |
| Asset-verification CLI model, sibling sheet and optional registry | [bin/asset_verify.rs:24](/Users/Yan/github/vikng-dev/overmatch/src/bin/asset_verify.rs:24), [bake.rs:958](/Users/Yan/github/vikng-dev/overmatch/src/bake.rs:958) | **E/tool input**, once per invocation; validates shipped integrity. |
| Asset-root environment/packaging selection | [assets.rs:102](/Users/Yan/github/vikng-dev/overmatch/src/assets.rs:102) | Startup location policy shared by direct readers and the asset server. |
| Hosted-server operator configuration, including endpoint, capacity and netcode key | [net/hosting.rs:176](/Users/Yan/github/vikng-dev/overmatch/src/net/hosting.rs:176) | Runtime operational input; startup/request boundaries, not art tuning. |

The following search hits do not supply the running game’s authored configuration:

- Test embeddings: `src/net/fixtures/battle-short.ron`; `src/ballistics/{field_casts,spall_cone}.pinned`; `src/tests/fixtures/uastc_16x16_mipped.ktx2`; `.agents/blender/fixture_tank.py`; `Cargo.lock`; backend JSON contract examples; shader text inspected by verification tests. Representative readers are [field_casts.rs:88](/Users/Yan/github/vikng-dev/overmatch/src/ballistics/field_casts.rs:88), [spec.rs:3249](/Users/Yan/github/vikng-dev/overmatch/src/spec.rs:3249), [quit.rs:469](/Users/Yan/github/vikng-dev/overmatch/src/quit.rs:469), [matchmaking/contract.rs:151](/Users/Yan/github/vikng-dev/overmatch/src/net/matchmaking/contract.rs:151) and [terrain_shadow.rs:557](/Users/Yan/github/vikng-dev/overmatch/src/terrain_shadow.rs:557).
- Repeated `include_str!` uses of shipped sheets/driving data inside tests are additional validation consumers of inputs already listed.
- `.blend` files and source texture inputs feed asset tooling; the game reads their generated artifacts. Licences and credit files are not runtime tuning inputs.
- No game-runtime reader was found for the tree’s `fir_1.lod.json`; the certificate path currently addresses the selected tank.
- `diesel_unedited.ogg` is not selected by current runtime declarations. Terrain folders containing only credit files likewise do not constitute loaded texture packs.
- `/proc` and cgroup reads are OS observations, not authored game inputs.

**File watching, verified against Bevy source**

The checked tree uses Bevy `0.19.1`, within the requested `0.19` line. File watching is **not enabled anywhere in the application setup inspected**.

The required public feature is **`bevy/file_watcher`**. Overmatch enables `debug`, `jpeg` and `basis-universal` at [Cargo.toml:28](/Users/Yan/github/vikng-dev/overmatch/Cargo.toml:28). Bevy’s `debug` feature does not imply its `dev` feature; `file_watcher` is opt-in. This is verified in the installed, versioned [Bevy manifest](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy-0.19.1/Cargo.toml:2776), with forwarding through [bevy_internal](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_internal-0.19.1/Cargo.toml:262) into [bevy_asset](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_asset-0.19.1/Cargo.toml:40).

The runtime setting is **`AssetPlugin::watch_for_changes_override`**, defaulting to `None`. The actual selection in [bevy_asset’s source](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_asset-0.19.1/src/lib.rs:365) is:

```rust
let watch = self
    .watch_for_changes_override
    .unwrap_or(cfg!(feature = "watch"));
```

Overmatch’s shared setup at [lib.rs:279](/Users/Yan/github/vikng-dev/overmatch/src/lib.rs:279) sets only:

```rust
AssetPlugin {
    file_path: root.to_string_lossy().into_owned(),
    ..default()
}
```

No production watch override or explicit asset-reload call was found. Enabling the feature supplies the watcher; `watch_for_changes_override: Some(true)` can make the runtime choice explicit. Neither will watch ordinary `include_str!` data or direct `std::fs` readers.

The default file-source debounce is **DERIVED from source: 300 milliseconds**, at [bevy_asset/io/source.rs:308](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_asset-0.19.1/src/io/source.rs:308). That is one part of latency, not a measured save-to-visible result.

**Construction boundaries that the migration must preserve**

`TankBlueprint` owns admitted spec/geometry through retained shared values at [bake.rs:351](/Users/Yan/github/vikng-dev/overmatch/src/bake.rs:351). Startup creates it from the embedded sheet and runtime sim artifact at [bake.rs:883](/Users/Yan/github/vikng-dev/overmatch/src/bake.rs:883). Spawn then initializes components synchronously from that construction at [tank/spawn.rs:269](/Users/Yan/github/vikng-dev/overmatch/src/tank/spawn.rs:269). Reloading the async `TankSpec` asset alone cannot change that source.

The current world reset also reuses startup construction: [world.rs:46](/Users/Yan/github/vikng-dev/overmatch/src/world.rs:46) reconstructs mutable environment entities while retaining terrain and shared assets. “Apply on reset” therefore requires refreshing the retained scenario inputs before reconstruction; the present reset is not already a file reload.

Finally, [net/protocol.rs:307](/Users/Yan/github/vikng-dev/overmatch/src/net/protocol.rs:307) caches content identity in a process-wide `OnceLock`. **Decision:** fingerprinted data stays fixed for an existing peer session. Externalizing it later requires the admitted bytes, construction and renewed peer identity to agree. Fingerprint participation does not inherently require compilation—the runtime map and sim artifact already demonstrate that—but clearing parser caches alone would violate the current contract.

**Ranked first moves**

Every path and latency expectation here is **DERIVED**, not measured. For small RON changes, target **about one second**, with watcher debounce, validation and consumer refresh included.

| Rank | Files | Expected save-to-visible path |
|---|---|---|
| **First** | `assets/world/lighting.ron` | Save → watch → parse/admit → update sun light/transform → regenerate sky/environment products and affected terrain-shadow data → rendered frame. Broad visible payoff; refresh all lighting products together. |
| **Second** | Presentation fields of `assets/ui/settings.ron`, then `assets/ui/overlay.ron` | Save → watch → validate → refresh/rebuild mounted UI nodes, fonts, backgrounds and interaction colours → UI frame. Keep boot defaults and player settings ownership separate from page appearance. |
| **Third** | `assets/ui/sight.ron`, `assets/ui/graticule.ron` | Save → watch → validate → refresh sight resource/materials/widgets; regenerate graduation entities when needed → frame. Local sensitivity changes also refresh their input reader. |
| **Fourth** | `assets/input/drive_hud.ron`, `assets/ui/death_screen.ron`; presentation fields of `spawn_map.ron` | Save → validate → refresh mounted HUD/screen components → frame. Keep `spawn_map.limit_fraction` on the simulation-reset path. |
| **Fifth** | `assets/materials/visuals.ron`, selected `.livery.ron` | Save → validate → replace surface/palette data → invalidate cached looks/coats → rewrite existing materials → frame. Larger cache-refresh surface than the look files. |
| **Sixth** | `assets/vfx/presentation.ron`, then `assets/vfx/muzzle.ron` | Save → validate → refresh view materials/parameters → next effect/frame. Muzzle data additionally needs its derived registry copies refreshed or relocated to presentation ownership. |
| **Seventh** | Runtime WGSL, then same-path textures/fonts/audio | Save → asset watch/reload → shader compilation or decode/upload → refresh dependent consumers → draw/playback. **DERIVED latency is variable**; no one-second claim is supported for arbitrary asset replacements. |
| **After presentation** | Driving, Battle, terrain, construction and game-state data | Save → synchronous admission of a new snapshot → spawn/scenario reset; renew peer content identity where required → new construction. Existing replicated simulation state is never re-applied from the changed file. |