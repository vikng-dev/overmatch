# `PbrInput::directional_shadow_factor` is one scalar for every shadow-casting directional light

**LOCAL DRAFT, not filed.** Found 2026-11-16 while moving Overmatch's baked terrain sun shadow
(rendering only; the game is a multiplayer tank video game) onto this hook. VERIFIED against the
bevy_pbr 0.20.1 crate (`vendor/bevy_pbr-0.20.1-overmatch`, whose shading files carry no patch;
`OVERMATCH_PATCH.md`). Bevy `main` and its issue tracker were not checked: do both before sending.

## Mechanism (bevy_pbr 0.20.1)

- `PbrInput::directional_shadow_factor: f32` (`src/render/pbr_types.wesl:121-123`): "Multiplier
  applied to all directional lights before cascade and contact shadows, use this for custom shadow
  maps. Defaults to 1.0." `pbr_input_new` sets it to 1.0 (`:151`).
- `apply_pbr_lighting`'s directional loop (`src/render/pbr_functions.wesl:585`) multiplies the one
  scalar into light `i`'s shadow inside the shadow-receiver block, only while that light carries
  `DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT` (`:600-605`), before its contact shadow
  (`:607-612`) and its contribution (`:619`). The flag is set where `prepare_lights` gives the
  light shadow maps (`src/render/light.rs:1790-1803`). Point and spot lights never read the factor
  (`:478`, `:532`).

```wgsl
var shadow: f32 = 1.0;
if ((in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
        && (view_bindings::lights.directional_lights[i].flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
    shadow = shadows::fetch_directional_shadow(i, in.world_position, in.world_normal, view_z, in.frag_coord.xy);
    shadow *= in.directional_shadow_factor;
}
```

A custom shadow is cast toward ONE light: a heightfield baked toward the sun, a cloud layer's
shadow. The factor cannot say which light it occludes, so in a view holding two shadow-casting
directional lights a material must shade both with it or neither. A light that casts no shadow is
untouched either way.

## Suggested fix

A per-light selection, defaulting to today's behaviour:

1. **A mask beside the scalar (smallest).** `directional_shadow_factor_lights: u32`, bit `i` set
   when the factor applies to directional light `i`, `0xffffffffu` from `pbr_input_new`. The loop
   tests `(in.directional_shadow_factor_lights >> i) & 1u` before multiplying. One `u32` in
   `PbrInput`; `MAX_DIRECTIONAL_LIGHTS` (10 off WebGL, `src/render/light.rs:230-236`) fits it.
2. **A factor per light.** `directional_shadow_factors: array<f32, MAX_DIRECTIONAL_LIGHTS>`, all 1.0
   by default. More general (two custom shadows toward two lights), at nine more floats in every
   `PbrInput`.

Either way a material finds its light's index by comparing `direction_to_light` with the direction
it was baked for: the GPU light record carries no identity a material could match instead.

## Our side

A world that binds the terrain's field holds at most one shadow-casting directional light, the sun
the field was baked for; `src/terrain_shadow.rs` (`the_sun_is_the_only_shadow_caster`) asserts it
every frame and panics naming any other. Lights casting none are untouched: the pen cam stage's two
key lights cast no shadow (`src/damage_cam/pen_cam/stage.rs`), so the kill cam lesson view, which
sees the sun and those keys (`src/render_policy.rs`), keeps the terrain's shadow. The cost lands
the first time the game wants a second shadow-casting directional light beside the terrain.

## What fixing this unlocks for us

A second shadow-casting directional light beside the terrain: the one-sun assertion goes, and the
receiver law selects the sun's index by matching `direction_to_light` against the baked direction.
