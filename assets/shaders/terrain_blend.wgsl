// The ground's surface blend: bevy's standard PBR fragment for the BASE pack, then three masked
// layers mixed into its albedo, normal and ARM. The CPU half and every constant this reads out of
// `blend.blend_law` live in `src/terrain_blend.rs`; the blend law itself lives here.
//
// DERIVATIVES ARE TAKEN ONCE, UP FRONT. A layer whose weight is zero is not sampled, and that
// branch is not uniform across the quad — so every layer fetch is `textureSampleGrad` with
// gradients computed outside the branch. `textureSample`'s implicit derivatives are undefined
// under non-uniform control flow and would cost the skip its licence. The prints' near store is
// read only near the eye, so it is read at level 0 (`textureSampleLevel`), which needs none — and
// which is the only kind of read its loops may make.
//
// TRACK PRINTS (`src/track_prints.rs`, cosmetic terrain marks). Each pack names a ground type
// (`src/ground_types.rs`), and the prints read the ground by the map's PAINTED weights
// (`print_weights`), which the CPU reproduces for the belt sink. A near-store texel holds a press
// (R: the sole's pattern times how far into its press the link is) and the laying tank's ground
// pressure (G); its depth is the ground's `softness × pressure`, capped, times R (`print_depth`).
// The world layer's trodden coverage darkens and roughens everywhere; inside the near store's
// window the depth's gradient bends the normal, and up to the player's relief distance the view
// ray is marched into the depth (parallax), and from where it lands toward the sun (the rut's
// self-shadow). Both offset the UV and the world position BEFORE the standard PBR input is built,
// so the base pack and the layers shift together.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_bindings,
    pbr_functions,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types,
    mesh_types,
    mesh_view_bindings as view_bindings,
    mesh_view_types,
    lighting,
    shadows,
}

struct TerrainBlendParams {
    // xyz: each layer's UV multiplier over the mesh's own UV. w: the mesh's UV per world metre.
    layer_uv_scale: vec4<f32>,
    // World XZ to mask UV: uv = world.xz * xy + zw.
    mask_map: vec4<f32>,
    // x: AO bias, y: blend window, z: skip threshold, w: unused.
    blend_law: vec4<f32>,
    // x: macro-tint periods per metre, y: macro-tint amplitude. zw: unused.
    macro_tint: vec4<f32>,
    // Per slot (x the base, yzw the layers), the ground type each pack names: print depth per kPa
    // of ground pressure (m), depth cap (m), albedo gain, roughness gain.
    print_softness: vec4<f32>,
    print_cap: vec4<f32>,
    print_albedo: vec4<f32>,
    print_roughness: vec4<f32>,
    // The near store: xy its window's low corner (world XZ, m), z its side (m), w its texel (m).
    print_near: vec4<f32>,
    // World XZ to world-layer UV: uv = world.xz * xy + zw.
    print_world: vec4<f32>,
    // x: the ground pressure (kPa) a full G channel stands for, y: window fade (m), z/w: pixels per
    // texel at which the near normal is gone / whole.
    print_law: vec4<f32>,
    // x: relief level (0 lighting only, 1 parallax, 2 parallax and the sun's shadow), y: the
    // distance relief reaches (m), z: the distance it fades to lighting only over (m).
    print_relief: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> blend: TerrainBlendParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var layer_albedo_texture: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var layer_albedo_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var layer_normal_texture: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var layer_normal_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var layer_arm_texture: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var layer_arm_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var masks_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var masks_sampler: sampler;
// The prints' stores. No samplers of their own: the near store wraps, so it is read through the
// layer albedo's repeating sampler; the world layer is stretched once, like the mask, and read
// through the mask's.
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var print_near_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(110) var print_world_texture: texture_2d<f32>;

// Layers, and layers plus the base pack. The mask has three channels; index 0 of the weight set is
// always the base.
const LAYERS: u32 = 3u;
const SLOTS: u32 = 4u;

// The macro tint's second octave, as a multiple of the first. NON-INTEGER, so the two octaves
// share no period and their sum does not repeat anywhere inside a map.
const MACRO_OCTAVE_RATIO: f32 = 2.7;
// Peak of the two-octave sum: the octaves are 1 and 1/2, each in 0..1.
const MACRO_PEAK: f32 = 1.5;

// How much of a print's press reads as fully trodden: a press of half or more darkens and roughens
// the ground fully. ASSUMED: the darkening fades in with the first half of a link's press, so a
// fresh print does not pop dark before it sinks.
const FOOTPRINT_GAIN: f32 = 2.0;
// The parallax march's step limits. ASSUMED minimum; DERIVED maximum: the tuning panel's
// 0.3 m depth cap at PARALLAX_MIN_DOWN spans at most 6 m, or 256 half-texel steps.
const PARALLAX_STEPS_MIN: f32 = 4.0;
const PARALLAX_STEPS_MAX: f32 = 256.0;
// The least depth per unit of view ray the march accepts: views flatter than about 3° to the ground
// march as if at 3°. ASSUMED.
const PARALLAX_MIN_DOWN: f32 = 0.05;
// The sun march's steps, from where the view ray lands up out of the rut toward the sun. ASSUMED,
// within the approaches note's 4–8.
const SUN_STEPS: u32 = 6u;
// How deep (as a share of the landing depth, m at least) the ground must stand over the sun ray to
// shade it fully: the shadow's soft edge. ASSUMED.
const SUN_SOFTNESS_SHARE: f32 = 0.25;
const SUN_SOFTNESS_MIN_M: f32 = 0.003;
// The sun lower than this over the ground's own plane (its sine) casts no rut shadow: the plane's
// own lighting already has it dark. ASSUMED.
const SUN_MIN_SINE: f32 = 0.02;

// One lattice corner's value, in 0..1.
fn macro_hash(cell: vec2<f32>) -> f32 {
    return fract(sin(dot(cell, vec2(127.1, 311.7))) * 43758.5453123);
}

// Smooth value noise in 0..1: four hashed lattice corners, interpolated on a smoothstep so the
// field is continuous across cells. Read at world scale, where the lattice indices stay small
// enough for `sin` to hash without precision loss.
fn macro_noise(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(macro_hash(cell), macro_hash(cell + vec2(1.0, 0.0)), w.x),
        mix(macro_hash(cell + vec2(0.0, 1.0)), macro_hash(cell + vec2(1.0, 1.0)), w.x),
        w.y,
    );
}

// The print's ground weights from the mask's painted weights, in slot order: the base pack where
// the mask is silent, each layer by its painted weight, normalised. `terrain_blend::print_weights`
// is this law, line for line, for the belt sink.
fn print_weights(mask: vec3<f32>) -> vec4<f32> {
    let base = 1.0 - saturate(mask.r + mask.g + mask.b);
    let weights = max(vec4(base, mask.r, mask.g, mask.b), vec4(0.0));
    return weights / max(dot(weights, vec4(1.0)), 1e-6);
}

// The depth (m) a full press reaches under a ground pressure of `kpa`, on ground of print weights
// `ground`: each slot's `softness × pressure`, capped, blended. `terrain_blend::print_depth_m`.
fn print_depth(ground: vec4<f32>, kpa: f32) -> f32 {
    return dot(ground, min(blend.print_cap, blend.print_softness * kpa));
}

// The near store at world XZ `p`: R the press, G the ground pressure over its full scale.
fn near_texel(p: vec2<f32>) -> vec2<f32> {
    // Repeating addresses the torus, not another copy of the world. A grazing march can leave
    // the resident window; its wrapped storage there belongs to unrelated ground.
    let local = p - blend.print_near.xy;
    if any(local < vec2(0.0)) || any(local >= vec2(blend.print_near.z)) {
        return vec2(0.0);
    }
    return textureSampleLevel(
        print_near_texture, layer_albedo_sampler, p / blend.print_near.z, 0.0).rg;
}

// World-space ray motion per metre of VERTICAL print depth. The depth law and the belt sink
// lower the height field vertically; on a sloped plane its normal separation is N.y * depth.
// Keeping the entire ray vector makes the UV's XZ and the PBR position the same landing.
fn vertical_depth_ray(direction: vec3<f32>, normal: vec3<f32>, normal_speed: f32) -> vec3<f32> {
    return direction * (normal.y / normal_speed);
}

// How far (m) the ground is sunk at world XZ `p`, on ground of print weights `ground`.
fn sunk_at(p: vec2<f32>, ground: vec4<f32>) -> f32 {
    let texel = near_texel(p);
    return print_depth(ground, texel.g * blend.print_law.x) * texel.r;
}

// How much the prints' near store speaks at world XZ `p`, 0..1: whole inside its window and where
// a texel spans enough pixels, fading to nothing at the window's edge and as a texel shrinks below
// a couple of pixels, where its gradient would alias. `ddx`/`ddy` are `p`'s screen derivatives. A
// pixel's size is its footprint's AREA, not its long axis: at a low view a pixel stretches far
// along the ground and stays narrow across it, and the long axis would fade a print 10 m off.
fn near_weight(p: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> f32 {
    let half = 0.5 * blend.print_near.z;
    let centre = blend.print_near.xy + vec2(half);
    let inside = half - max(abs(p.x - centre.x), abs(p.y - centre.y));
    let window = saturate(inside / blend.print_law.y);
    let pixel_m = sqrt(abs(ddx.x * ddy.y - ddx.y * ddy.x));
    let px_per_texel = blend.print_near.w / max(pixel_m, 1e-6);
    let detail = saturate(
        (px_per_texel - blend.print_law.z) / (blend.print_law.w - blend.print_law.z));
    return window * detail;
}

// How much of the prints' relief (parallax and the sun) is drawn at `distance` m from the eye:
// whole up to the player's distance less its fade, nothing beyond it.
fn relief_weight(distance: f32) -> f32 {
    let reach = blend.print_relief.y;
    let fade = max(blend.print_relief.z, 1e-3);
    return saturate((reach - distance) / fade);
}

// The parallax march: from world XZ `p` on the surface, down the view ray whose surface-plane
// motion per metre of depth is `lateral`, to where it meets the sunk ground (depth scaled by
// `scale`). Returns the vertical depth (m) it lands at; the landing's world-XZ
// point is `p + lateral * depth`. `bound` is the deepest the ground can be here.
fn parallax_depth(
    p: vec2<f32>, lateral: vec2<f32>, ground: vec4<f32>, scale: f32, bound: f32,
) -> f32 {
    var h_prev = sunk_at(p, ground) * scale;
    if h_prev <= 0.0 {
        return 0.0;
    }
    let travel = length(lateral) * bound / (0.5 * blend.print_near.w);
    let steps = clamp(ceil(travel), PARALLAX_STEPS_MIN, PARALLAX_STEPS_MAX);
    let dz = bound / steps;
    var z_prev = 0.0;
    for (var i = 1.0; i <= steps; i += 1.0) {
        let z = dz * i;
        let h = sunk_at(p + lateral * z, ground) * scale;
        if z >= h {
            // Between the last step above the ground and this one below it, where the two lines
            // cross.
            let above = h_prev - z_prev;
            let below = z - h;
            return mix(z_prev, z, above / max(above + below, 1e-6));
        }
        z_prev = z;
        h_prev = h;
    }
    return bound;
}

// The sun's visibility (0..1) at the landing: world XZ `p` on the surface plane at depth `depth`,
// marched toward the sun, whose surface-plane motion per metre of rise is `lateral`, until the ray
// leaves the rut. Where the ground there is sunk less than the ray is deep, the ground stands over
// the ray and shades it, softly; a rut's open floor, sunk as deep as the ray, does not.
// `scale` fades both the relief height and the shadow strength to lighting only.
fn sun_visibility(
    p: vec2<f32>, depth: f32, lateral: vec2<f32>, ground: vec4<f32>, scale: f32,
) -> f32 {
    let softness = max(depth * SUN_SOFTNESS_SHARE, SUN_SOFTNESS_MIN_M);
    var occluded = 0.0;
    for (var k = 1u; k <= SUN_STEPS; k++) {
        let rise = depth * f32(k) / f32(SUN_STEPS);
        let h = sunk_at(p + lateral * rise, ground) * scale;
        occluded = max(occluded, ((depth - rise) - h) / softness);
    }
    return 1.0 - scale * saturate(occluded);
}

// The sun's direct light as `apply_pbr_lighting` adds it for `pbr_input`, shadow map and exposure
// included: what a rut's self-shadow takes away. The sun is the scene's first directional light;
// the terrain material enables neither clearcoat nor anisotropy, so the base layer is the input.
fn sun_direct(pbr_input: pbr_types::PbrInput) -> vec3<f32> {
    if view_bindings::lights.n_directional_lights == 0u {
        return vec3(0.0);
    }
    let base = pbr_input.material.base_color.rgb;
    let metallic = pbr_input.material.metallic;
    let perceptual_roughness = pbr_input.material.perceptual_roughness;
    let NdotV = max(dot(pbr_input.N, pbr_input.V), 0.0001);
    var input: lighting::LightingInput;
    input.layers[lighting::LAYER_BASE].NdotV = NdotV;
    input.layers[lighting::LAYER_BASE].N = pbr_input.N;
    input.layers[lighting::LAYER_BASE].R = reflect(-pbr_input.V, pbr_input.N);
    input.layers[lighting::LAYER_BASE].perceptual_roughness = perceptual_roughness;
    input.layers[lighting::LAYER_BASE].roughness =
        lighting::perceptualRoughnessToRoughness(perceptual_roughness);
    input.P = pbr_input.world_position.xyz;
    input.V = pbr_input.V;
    input.diffuse_color = pbr_functions::calculate_diffuse_color(
        base,
        metallic,
        pbr_input.material.specular_transmission,
        pbr_input.material.diffuse_transmission,
    );
    input.metallic = metallic;
    input.F0_dielectric = pbr_functions::calculate_F0_dielectric(pbr_input.material.reflectance);
    input.F0_metallic = base;
    input.F_ab = lighting::F_AB(perceptual_roughness, NdotV);
    var shadow = 1.0;
    if (pbr_input.flags & mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
        && (view_bindings::lights.directional_lights[0].flags
            & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u {
        let view_z = dot(vec4<f32>(
            view_bindings::view.view_from_world[0].z,
            view_bindings::view.view_from_world[1].z,
            view_bindings::view.view_from_world[2].z,
            view_bindings::view.view_from_world[3].z
        ), pbr_input.world_position);
        shadow = shadows::fetch_directional_shadow(
            0u, pbr_input.world_position, pbr_input.world_normal, view_z, pbr_input.frag_coord.xy);
    }
    return lighting::directional_light(0u, &input, true) * shadow * view_bindings::view.exposure;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // Derivatives of the surface as the mesh draws it, before any print moves it: continuous across
    // a rut's edge, where the moved UV is not.
    let world_xz = in.world_position.xz;
    let ddx_xz = dpdx(world_xz);
    let ddy_xz = dpdy(world_xz);
    let ddx_uv = dpdx(in.uv);
    let ddy_uv = dpdy(in.uv);

    let mask = textureSample(
        masks_texture,
        masks_sampler,
        world_xz * blend.mask_map.xy + blend.mask_map.zw,
    ).rgb;
    let painted = array<f32, LAYERS>(mask.r, mask.g, mask.b);
    let ground = print_weights(mask);

    // The prints' world layer: what share of this ground a sole has covered. Uniform control
    // flow, so its implicit derivatives pick the mip.
    let trodden = textureSample(
        print_world_texture,
        masks_sampler,
        world_xz * blend.print_world.xy + blend.print_world.zw,
    ).r;

    // TRACK PRINTS. Far from the eye the world layer's coverage is all there is; inside the near
    // store its sharp footprint takes over, its depth's gradient tilts the normal, and within the
    // relief distance the view lands on the sunk ground and the sun is marched across the rut.
    let near = near_weight(world_xz, ddx_xz, ddy_xz);
    let geometric_normal = normalize(in.world_normal);
    var landed_xz = world_xz;
    var landed_depth = 0.0;
    var landed_offset = vec3(0.0);
    var sun = 1.0;
    var footprint = trodden;
    var depth_slope = vec3(0.0);
    if near > 0.0 {
        let relief = near * relief_weight(distance(view_bindings::view.world_position, in.world_position.xyz));
        let bound = dot(ground, blend.print_cap) * relief;
        if blend.print_relief.x >= 0.5 && bound > 1e-4 {
            // The view ray into the vertically lowered height field.
            let into = normalize(in.world_position.xyz - view_bindings::view.world_position);
            let down = max(-dot(into, geometric_normal), PARALLAX_MIN_DOWN);
            let ray = vertical_depth_ray(into, geometric_normal, down);
            let lateral = ray.xz;
            landed_depth = parallax_depth(world_xz, lateral, ground, relief, bound);
            landed_offset = ray * landed_depth;
            landed_xz = world_xz + landed_offset.xz;
            if blend.print_relief.x >= 1.5 && landed_depth > 0.0
                && view_bindings::lights.n_directional_lights > 0u {
                let to_sun = view_bindings::lights.directional_lights[0].direction_to_light;
                let rise = dot(to_sun, geometric_normal);
                if rise > SUN_MIN_SINE {
                    let sun_lateral = vertical_depth_ray(to_sun, geometric_normal, rise).xz;
                    sun = sun_visibility(landed_xz, landed_depth, sun_lateral, ground, relief);
                }
            }
        }
        let texel_m = blend.print_near.w;
        let here = near_texel(landed_xz);
        // d(depth)/dx and d(depth)/dz, world space: the surface DESCENDS toward a deeper texel.
        let gradient = vec2(
            sunk_at(landed_xz + vec2(texel_m, 0.0), ground)
                - sunk_at(landed_xz - vec2(texel_m, 0.0), ground),
            sunk_at(landed_xz + vec2(0.0, texel_m), ground)
                - sunk_at(landed_xz - vec2(0.0, texel_m), ground),
        ) / (2.0 * texel_m);
        depth_slope = vec3(gradient.x, 0.0, gradient.y) * near;
        footprint = mix(trodden, saturate(here.r * FOOTPRINT_GAIN), near);
    }

    // The surface as the view meets it, on the view ray, before the standard input samples the
    // base pack. The UV and the PBR position share the landing's actual XZ.
    var landed = in;
    let shift = landed_xz - world_xz;
    landed.uv = in.uv + shift * blend.layer_uv_scale.w;
    landed.world_position = vec4(
        in.world_position.xyz + landed_offset,
        in.world_position.w,
    );

    // The base pack, through the whole standard path: its albedo, its normal map resolved against
    // the mesh tangents, its ARM, and every non-texture material parameter.
    var pbr_input = pbr_input_from_standard_material(landed, is_front);

    // The material's own FACTORS, before the base pack's maps were multiplied into them
    // (`pbr_input.material` already carries that product). Every layer is multiplied by the same
    // pair, so the four packs answer to one material.
    let base_color_factor = pbr_bindings::material.base_color.rgb;
    let roughness_factor = pbr_bindings::material.perceptual_roughness;

    let double_sided = (pbr_input.material.flags
        & pbr_types::STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT) != 0u;
    let TBN = pbr_functions::calculate_tbn_mikktspace(pbr_input.world_normal, in.world_tangent);

    // Gather: the base pack's own values, then each layer the mask actually paints. A skipped
    // layer keeps a zero raw weight, which the law below carries through to a zero final weight.
    var albedo = array<vec3<f32>, SLOTS>();
    var normal = array<vec3<f32>, SLOTS>();
    var occlusion = array<f32, SLOTS>();
    var roughness = array<f32, SLOTS>();
    var raw = array<f32, SLOTS>();

    albedo[0] = pbr_input.material.base_color.rgb;
    normal[0] = pbr_input.N;
    occlusion[0] = pbr_input.diffuse_occlusion.r;
    roughness[0] = pbr_input.material.perceptual_roughness;
    var covered = 0.0;
    for (var i = 0u; i < LAYERS; i++) {
        covered += painted[i];
    }
    raw[0] = 1.0 - saturate(covered);

    for (var i = 0u; i < LAYERS; i++) {
        let weight = painted[i];
        if weight <= blend.blend_law.z {
            continue;
        }
        let scale = blend.layer_uv_scale[i];
        let uv = landed.uv * scale;
        let ddx = ddx_uv * scale;
        let ddy = ddy_uv * scale;
        let slot = i + 1u;
        let layer = i32(i);
        albedo[slot] = base_color_factor * textureSampleGrad(
            layer_albedo_texture, layer_albedo_sampler, uv, layer, ddx, ddy).rgb;
        let tangent_normal = textureSampleGrad(
            layer_normal_texture, layer_normal_sampler, uv, layer, ddx, ddy).rgb;
        normal[slot] = pbr_functions::apply_normal_mapping(
            pbr_input.material.flags, TBN, double_sided, is_front, tangent_normal);
        let arm = textureSampleGrad(
            layer_arm_texture, layer_arm_sampler, uv, layer, ddx, ddy).rgb;
        occlusion[slot] = arm.r;
        roughness[slot] = roughness_factor * arm.g;
        raw[slot] = weight;
    }

    // THE BLEND LAW: bias every weight by its own pack's AO, cut everything more than a window
    // behind the leader, normalize.
    var biased = array<f32, SLOTS>();
    var leader = 0.0;
    for (var i = 0u; i < SLOTS; i++) {
        biased[i] = raw[i] * (1.0 + blend.blend_law.x * occlusion[i]);
        leader = max(leader, biased[i]);
    }
    var weights = array<f32, SLOTS>();
    var total = 0.0;
    for (var i = 0u; i < SLOTS; i++) {
        weights[i] = biased[i] * max(1.0 - (leader - biased[i]) / blend.blend_law.y, 0.0);
        total += weights[i];
    }

    var blended_albedo = vec3(0.0);
    var blended_normal = vec3(0.0);
    var blended_occlusion = 0.0;
    var blended_roughness = 0.0;
    for (var i = 0u; i < SLOTS; i++) {
        let weight = weights[i] / total;
        blended_albedo += albedo[i] * weight;
        // Normals blend as VECTORS, in world space, and are renormalized below.
        blended_normal += normal[i] * weight;
        blended_occlusion += occlusion[i] * weight;
        blended_roughness += roughness[i] * weight;
    }

    // THE MACRO TINT. Every pack repeats at its own authored metres, and a repeat is caught by its
    // large-scale BRIGHTNESS pattern rather than by its detail — so a low-frequency gain over the
    // blended albedo breaks the read without touching normal, ARM or the packs themselves. Applied
    // to the MIX, which is the only place the base pack can be reached: its maps run through the
    // standard path above, outside the layer loop.
    //
    // A function of world XZ and nothing else — no camera term, so it does not swim under motion.
    let macro_uv = in.world_position.xz * blend.macro_tint.x;
    let macro_field = macro_noise(macro_uv) + 0.5 * macro_noise(macro_uv * MACRO_OCTAVE_RATIO);
    // 0..MACRO_PEAK to -1..1, then a gain of 1 ± amplitude. The amplitude is bounded below 1, so
    // the gain is strictly positive whatever the field returns.
    let macro_signed = macro_field * (2.0 / MACRO_PEAK) - 1.0;
    blended_albedo *= 1.0 + blend.macro_tint.y * macro_signed;

    // The trodden ground's look, by the same painted weights as its depth.
    blended_albedo *= mix(1.0, dot(ground, blend.print_albedo), footprint);
    blended_roughness *= mix(1.0, dot(ground, blend.print_roughness), footprint);
    // Mikkelsen's surface gradient: the ground's height is `−depth`, so its normal leans along
    // the depth's gradient, projected into the surface's own plane.
    let surface_normal = normalize(blended_normal);
    let leaned = surface_normal + depth_slope - surface_normal * dot(surface_normal, depth_slope);

    pbr_input.material.base_color = vec4(blended_albedo, pbr_input.material.base_color.a);
    pbr_input.N = normalize(leaned);
    pbr_input.diffuse_occlusion = vec3(blended_occlusion);
    pbr_input.material.perceptual_roughness = blended_roughness;
    // Metallic is not blended: the material's metallic factor is hard zero, so every pack's B
    // channel multiplies out to a dielectric ground whatever the mask says.

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    // The rut's self-shadow takes the sun's direct light, and only the sun's, where the ground
    // stands over the ray to it.
    if sun < 1.0 {
        out.color = vec4(out.color.rgb - (1.0 - sun) * sun_direct(pbr_input), out.color.a);
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
