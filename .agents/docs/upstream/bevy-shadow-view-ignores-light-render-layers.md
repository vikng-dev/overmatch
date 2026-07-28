# Upstream: shadow views never inherit the light's `RenderLayers`, so off-layer meshes never cast

Status: **DO NOT FILE — already reported and fixed upstream, but unreleased.** Kept as the mechanism
record + vendored-patch rationale.
Found: 2026-07-28, Overmatch, while putting meshes on non-zero render layers. Root-caused from the
vendored source, not inferred; the main-world half of the contradiction is reproduced in-tree by
`tests/bevy_shadow_view_render_layers.rs`.

## Upstream status (checked 2026-07-28)

- Issue: bevyengine/bevy **#24792** "Directional lights shadows not rendered for non default render
  layers" (opened 2026-06-28, closed 2026-07-08). Reported as directional-only; it is all three
  shadow-casting light types.
- Fix: **PR #24797** "Ensure `RenderLayers` is a Component on every `ExtractedLight` and its
  `ShadowView`/`ExtractedView`" (kfc35, opened 2026-06-28, merged 2026-07-08, merge commit
  `49cca9a4c8cb`), milestone **0.19.1**. One file, `crates/bevy_pbr/src/render/light.rs`.
- crates.io still ships only **0.19.0** (`bevy` 0.19.0 is `newest_version`; 0.19.1 exists only as a
  milestone) — exactly the same situation as the `check_dir_light_mesh_visibility` cascade panic in
  `bevy-cascade-count-stale-local-parallel.md`, and the reason both fixes live in `vendor/`.
- Distinct from bevyengine/bevy **#23264** "Regression: RenderLayers do not work for Lights" (open,
  PR **#23925** open + blocked). That one is about a light *illuminating* meshes outside its layers
  and is shader-side; #24797's own description calls out the separation. Our defect is purely about
  which casters reach the shadow phase.

## Mechanism (verified against bevy_pbr-0.19.0 + bevy_light-0.19.0 source)

Bevy filters shadow casters **twice**, and the two filters disagree about what they are filtering
against.

### Filter 1 — main world, against the LIGHT's mask (correct)

`check_dir_light_mesh_visibility` (bevy_light `lib.rs:336`, PostUpdate
`SimulationLightSystems::CheckLightVisibility`) reads the light's own `Option<&RenderLayers>`
(`lib.rs:343`), defaults it (`lib.rs:400`), and tests each candidate caster against it:

```rust
let entity_mask = maybe_entity_mask.unwrap_or_default();   // lib.rs:423
if !view_mask.intersects(entity_mask) {                    // lib.rs:424
    return;
}
```

The caster query (`lib.rs:348-364`) is `Without<NotShadowCaster>, With<Mesh3d>` — it is *not*
restricted to entities any camera can see. Better: casters accepted here are force-marked visible
(`lib.rs:454-457` collect, `lib.rs:485-497` `view_visibility.set_visible()`) precisely so extraction
retains meshes the camera cannot see. Casting from off-camera geometry is a deliberate,
load-bearing property. `check_point_light_mesh_visibility` does the same for point (`lib.rs:575`,
`597-598`) and spot (`lib.rs:666`, `689-690`).

So after PostUpdate, a mesh on layer 1 lit by a light masked `[0, 1]` is *in* the light's
`CascadesVisibleEntities` / `CubemapVisibleEntities`, and `extract_lights` copies it into the render
world.

### Filter 2 — render world, against the shadow VIEW's mask (structurally broken)

`queue_shadows` (bevy_pbr `light.rs:2505`) then re-tests every one of those casters:

```rust
view_light_entities: Query<(&LightEntity, &ExtractedView, Option<&RenderLayers>)>,   // light.rs:2512
...
let mesh_layers = mesh_instance.render_layers.as_ref().unwrap_or_default();          // light.rs:2582
let view_render_layers = maybe_view_render_layers.unwrap_or_default();               // light.rs:2583
if !view_render_layers.intersects(mesh_layers) {                                      // light.rs:2584
    continue;
}
```

`maybe_view_render_layers` is the mask of the **shadow view entity** — an internal, render-world-only
entity created by `prepare_lights`. Every one of them is spawned bare:

| shadow view | spawned | populated | gets `RenderLayers`? |
|---|---|---|---|
| directional cascade | `light.rs:1814`, `1820` (`spawn_empty`) | `light.rs:1869-1897` | **no** |
| point cubemap face (x6) | `light.rs:1483` (`spawn_empty`) | `light.rs:2064-2094` (`create_point_shadow_maps`) | **no** |
| spot | `light.rs:1560` (`spawn_empty`) | `light.rs:2150-2175` (`create_spot_shadow_map`) | **no** |

Nothing anywhere in 0.19.0 inserts a `RenderLayers` on a shadow view. `maybe_view_render_layers` is
therefore *always* `None`, always defaults to `RenderLayers::default()`, and always means layer 0.

The result: **a mesh on any non-zero render layer never casts a shadow, from any light, no matter
what the light's own mask says.** Filter 2 cannot do anything except wrongly reject casters that
filter 1 deliberately accepted — it has no other reachable behaviour. That is an internal
contradiction, not a contract: the light's mask is authoritative in `bevy_light` and then silently
overruled in `bevy_pbr` by an entity that has no mask to be authoritative with.

Two details confirm it is an oversight rather than intent:

- The query at `light.rs:2512` already asks for `Option<&RenderLayers>` on the view — the code was
  written expecting the view to carry one.
- `ExtractedDirectionalLight` already carries `render_layers` (`light.rs:124`, populated at
  `light.rs:815`) and `prepare_lights` already uses it to decide *which cameras* a directional light
  affects (`light.rs:1342`, `1649`, `1750`, `1771`). The mask is sitting right there at the
  construction site; it just never makes it onto the view. `ExtractedPointLight` (`light.rs:79-96`)
  has no mask field at all, so for point and spot lights the mask does not even reach the render
  world.

Adjacent, and worth not confusing with this: `collect_gpu_culled_meshes` (bevy_pbr
`mesh.rs:2231-2255`) filters each light's `RenderShadowMapVisibleEntities` against
`Option<&RenderLayers>` on the *extracted light entity* — also always `None` in 0.19.0, but that
path is permissive when absent (`is_none_or`, `mesh.rs:2320`), so it silently accepts rather than
silently drops. Only filter 2 has the inverted default.

### Repro (as filed in #24792)

Camera, light and meshes all on layer 1 — no shadows; flip `LAYER` to 0 and they appear. The
sharper form, which isolates the contradiction rather than merely showing the symptom:

```rust
// bevy 0.19.0
let camera_layer = RenderLayers::layer(0);
let caster_layer = RenderLayers::layer(1);

commands.spawn((Camera3d::default(), transform_looking_at_scene(), camera_layer));
commands.spawn((
    DirectionalLight { shadow_maps_enabled: true, ..default() },
    Transform::default().looking_to(Vec3::new(-0.5, -1.0, -0.3), Vec3::Y),
    // Covers BOTH layers: the camera's (or the light would not be view-visible at all)
    // and the caster's.
    RenderLayers::from_layers(&[0, 1]),
));
commands.spawn((Mesh3d(cube), MeshMaterial3d(mat), caster_layer));      // the caster
commands.spawn((Mesh3d(plane), MeshMaterial3d(mat), camera_layer));     // the receiver
```

Stock 0.19.0 accepts the layer-1 cube in `check_dir_light_mesh_visibility` (it is in the light's
`CascadesVisibleEntities`, and its `ViewVisibility` is force-set) and then drops it in
`queue_shadows` — the plane renders unshadowed. Same outcome with `PointLight` or `SpotLight`.

`tests/bevy_shadow_view_render_layers.rs::light_visibility_accepts_a_caster_the_camera_cannot_see`
pins the first half of that as an executable assertion, with a layer-2 caster as the control that
proves the acceptance really is mask-driven. The second half cannot be executed headless: with
`WgpuSettings { backends: None }` bevy 0.19 never creates the `RenderApp`, so no shadow view is
built and `queue_shadows` never runs. Proving the drop requires a real adapter; CI has none.

## Local resolution

`vendor/bevy_pbr-0.19.0-scalar-math/src/render/light.rs`, every site tagged `// OVERMATCH PATCH:`
(the crate was already vendored for the `MeshUniform` alignment fix — see `OVERMATCH_PATCH.md`):

- `ExtractedPointLight` gains `pub render_layers: RenderLayers`, populated in `extract_lights` from
  the point/spot light entity's `Option<&RenderLayers>` (absent → `RenderLayers::default()`), with
  `Option<&RenderLayers>` added to both extract queries and `Changed<RenderLayers>` added to both
  `Or<(..)>` filters. The `Changed` part is load-bearing and easy to miss: point/spot shadow views
  are **cached** — `prepare_lights` only rebuilds them when the view list is empty or
  `Changed<ExtractedPointLight>` fires (`light.rs:1481`, `1501`, `1559`, `1575`) — so without
  re-extraction a runtime mask change would never reach an existing view.
- `create_point_shadow_maps` and `create_spot_shadow_map` insert `light.render_layers.clone()` on
  the view-light entity.
- The directional cascade loop inserts `light.render_layers.clone()` on each cascade's view-light
  entity, next to its `LightEntity::Directional`.

All three types are patched on purpose. A directional-only patch (which is how #24792 was reported)
looks like it works — the sun is what casts the shadows one notices — and leaves point and spot
quietly broken.

The mask copied is always the **light's**, never the camera's. Conflating them would break the
deliberate property established in filter 1: a caster outside the camera's layers must still cast.

### How this differs from upstream #24797

Same semantics for the three shadow-casting light types, smaller blast radius:

| | #24797 | ours |
|---|---|---|
| where the mask lives in the render world | a `RenderLayers` **component** on the extracted light entity; `ExtractedDirectionalLight::render_layers` **removed** | stays a field: kept on `ExtractedDirectionalLight`, added to `ExtractedPointLight` |
| `prepare_lights` queries | `&RenderLayers` (non-`Option`) added to the point/directional/rect light queries, every tuple index shifted | unchanged |
| rect lights | mask extracted too (unused — they have no shadows; upstream leaves a TODO) | untouched |
| public API change | yes (`ExtractedDirectionalLight` loses a public field) | none |

Upstream's shape is the better long-term design — attaching the mask as a component makes the
adjacent `collect_gpu_culled_meshes` light query (`mesh.rs:2233`) start working too. Ours is chosen
for a *vendored* crate: it does not renumber query tuples across ~20 call sites, so it re-applies
cleanly against a future 0.19.x, and it changes no public type. Both leave `queue_shadows`
untouched.

**Retirement:** drop the patch (and probably the whole vendor entry, pending the `MeshUniform`
question) when bevy 0.19.1 ships with #24797. `tests/bevy_shadow_view_render_layers.rs` is the
guard until then — it is a *forward* tripwire (it asserts the patch is present), unlike the inverted
`tests/bevy_ktx2_uastc_fallback.rs`, so a re-vendor that silently drops the patch fails CI.
`queue_shadows_still_filters_on_the_view_mask` is the anchor that changes shape when upstream's
version of the fix arrives.

## Artifacts

- `vendor/bevy_pbr-0.19.0-scalar-math/src/render/light.rs` — the patch, tagged `// OVERMATCH PATCH:`.
- `vendor/bevy_pbr-0.19.0-scalar-math/OVERMATCH_PATCH.md` — the vendored crate's own patch inventory.
- `tests/bevy_shadow_view_render_layers.rs` — source tripwire over all three construction sites,
  plus the executable main-world half of the contradiction.
