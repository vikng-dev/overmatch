import constants::MATERIAL_BIND_GROUP;

// The terrain's baked sun shadow (`src/terrain_shadow.rs` owns the bake, the record and the
// uniform): the receiver law, and the two material bindings it reads. A material that receives the
// shadow binds the field at 150 and its uniform at 151, and multiplies `visibility` at its lit
// position into `pbr_input.directional_shadow_factor` before `apply_pbr_lighting`.

struct TerrainShadowUniform {
    // xy: world XZ of texel (0, 0); z: texels per metre; w: metres per texel.
    map: vec4<f32>,
    // xy: the unit horizontal direction toward the sun; z: the sun's rise per horizontal metre;
    // w: 1 when the field is bound, 0 when absent.
    ray: vec4<f32>,
    // x: the sun's angular radius (rad); y: the least penumbra half-width (m).
    law: vec4<f32>,
};

// Rg32Float: R the occluder's height (m), G its horizontal distance along the sun (m), G < 0 none.
@group(MATERIAL_BIND_GROUP) @binding(150) var terrain_shadow_field: texture_2d<f32>;
@group(MATERIAL_BIND_GROUP) @binding(151) var<uniform> terrain_shadow: TerrainShadowUniform;

// THE RECEIVER LAW.
//
// The field's visibility (0..1) at `position`. Each of the four texels around the fragment holds the
// occluder its own ray toward the sun meets; that occluder's line toward the sun is carried to the
// fragment, which is lit above it. A texel whose occluder lies at or behind the fragment (or that
// records none) has nothing to say about it and is left out; the rest are blended by their bilinear
// weights. The penumbra is the sun's disc seen past the occluder's edge: an edge `depth` above the
// fragment's ray to the sun's centre, `reach` metres away horizontally, stands
// `depth · cos²(elevation) / reach` radians above it.
//
// Bevy multiplies the factor into a directional light's shadow inside its shadow-receiver block, at
// every distance and only while the light casts shadows: one scalar for all such lights
// (`pbr_types.wesl`, `pbr_functions.wesl`). The game keeps the baked sun the only one wherever a
// field is bound (`terrain_shadow::the_sun_is_the_only_shadow_caster`). It is silent outside the
// field.
fn visibility(position: vec3<f32>) -> f32 {
    if terrain_shadow.ray.w == 0.0 {
        return 1.0;
    }
    let size = vec2<i32>(textureDimensions(terrain_shadow_field));
    let grid = (position.xz - terrain_shadow.map.xy) * terrain_shadow.map.z;
    if any(grid < vec2<f32>(0.0)) || any(grid > vec2<f32>(size - 1)) {
        return 1.0;
    }
    let cell = min(vec2<i32>(floor(grid)), size - 2);
    let f = grid - vec2<f32>(cell);
    var depth = 0.0;
    var reach = 0.0;
    var weight = 0.0;
    for (var k = 0u; k < 4u; k++) {
        let corner = vec2<u32>(k & 1u, k >> 1u);
        let occluder = textureLoad(terrain_shadow_field, cell + vec2<i32>(corner), 0).rg;
        // How far the fragment stands toward the sun from this texel, m.
        let ahead = dot(f - vec2<f32>(corner), terrain_shadow.ray.xy) * terrain_shadow.map.w;
        if occluder.y >= 0.0 && occluder.y > ahead {
            let w = select(1.0 - f.x, f.x, corner.x == 1u) * select(1.0 - f.y, f.y, corner.y == 1u);
            depth += w * (occluder.x - (occluder.y - ahead) * terrain_shadow.ray.z - position.y);
            reach += w * (occluder.y - ahead);
            weight += w;
        }
    }
    if weight <= 0.0 {
        return 1.0;
    }
    let tan_elevation = terrain_shadow.ray.z;
    let half_width = max(
        terrain_shadow.law.x * (reach / weight) * (1.0 + tan_elevation * tan_elevation),
        terrain_shadow.law.y,
    );
    return 1.0 - smoothstep(-half_width, half_width, depth / weight);
}
