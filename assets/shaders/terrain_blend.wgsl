// The ground's surface blend: bevy's standard PBR fragment for the BASE pack, then three masked
// layers mixed into its albedo, normal and ARM. The CPU half and every constant this reads out of
// `blend.blend_law` live in `src/terrain_blend.rs`; the blend law itself lives here.
//
// DERIVATIVES ARE TAKEN ONCE, UP FRONT. A layer whose weight is zero is not sampled, and that
// branch is not uniform across the quad — so every layer fetch is `textureSampleGrad` with
// gradients computed outside the branch. `textureSample`'s implicit derivatives are undefined
// under non-uniform control flow and would cost the skip its licence. The prints' near store is
// read only near the eye, so it is read at level 0 (`textureSampleLevel`), which needs none.
//
// TRACK PRINTS (`src/track_prints.rs`). Each pack's response (depth, albedo gain, roughness gain)
// is blended by the same final weights as the packs. The world layer's trodden coverage darkens
// and roughens everywhere; inside the near store's window a sole's clearance over the ground
// becomes a depth, `max(0, depth − clearance)`, whose gradient bends the normal. The near store
// fades out at its window's edge and where a texel shrinks below a couple of pixels.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_bindings,
    pbr_functions,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types,
}

struct TerrainBlendParams {
    // xyz: each layer's UV multiplier over the mesh's own UV. w: unused.
    layer_uv_scale: vec4<f32>,
    // World XZ to mask UV: uv = world.xz * xy + zw.
    mask_map: vec4<f32>,
    // x: AO bias, y: blend window, z: skip threshold, w: unused.
    blend_law: vec4<f32>,
    // x: macro-tint periods per metre, y: macro-tint amplitude. zw: unused.
    macro_tint: vec4<f32>,
    // Per slot (x the base, yzw the layers): print depth (m), albedo gain, roughness gain.
    print_depth: vec4<f32>,
    print_albedo: vec4<f32>,
    print_roughness: vec4<f32>,
    // The near store: xy its window's low corner (world XZ, m), z its side (m), w its texel (m).
    print_near: vec4<f32>,
    // World XZ to world-layer UV: uv = world.xz * xy + zw.
    print_world: vec4<f32>,
    // x: the clearance range the near press spans (m), y: window fade (m), z/w: pixels per texel
    // at which the near normal is gone / whole.
    print_law: vec4<f32>,
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

// How much the prints' near store speaks at world XZ `p`, 0..1: whole inside its window and where
// a texel spans enough pixels, fading to nothing at the window's edge and as a texel shrinks below
// a couple of pixels, where its gradient would alias. `ddx`/`ddy` are `p`'s screen derivatives.
fn near_weight(p: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> f32 {
    let half = 0.5 * blend.print_near.z;
    let centre = blend.print_near.xy + vec2(half);
    let inside = half - max(abs(p.x - centre.x), abs(p.y - centre.y));
    let window = saturate(inside / blend.print_law.y);
    let px_per_texel = blend.print_near.w / max(max(length(ddx), length(ddy)), 1e-6);
    let detail = saturate(
        (px_per_texel - blend.print_law.z) / (blend.print_law.w - blend.print_law.z));
    return window * detail;
}

// How far the ground sinks (m) under a near-store press `press`, on ground whose blended print
// depth is `depth`: a sole clearing the ground by `(1 − press) × range` sinks it by what is left.
fn sunk(press: f32, depth: f32) -> f32 {
    return max(depth - (1.0 - press) * blend.print_law.x, 0.0);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // The base pack, through the whole standard path: its albedo, its normal map resolved against
    // the mesh tangents, its ARM, and every non-texture material parameter.
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let mask = textureSample(
        masks_texture,
        masks_sampler,
        in.world_position.xz * blend.mask_map.xy + blend.mask_map.zw,
    ).rgb;
    let painted = array<f32, LAYERS>(mask.r, mask.g, mask.b);

    // The prints' world layer: what share of this ground a sole has covered. Uniform control
    // flow, so its implicit derivatives pick the mip; the near store's are taken here too.
    let world_xz = in.world_position.xz;
    let ddx_xz = dpdx(world_xz);
    let ddy_xz = dpdy(world_xz);
    let trodden = textureSample(
        print_world_texture,
        masks_sampler,
        world_xz * blend.print_world.xy + blend.print_world.zw,
    ).r;

    // The material's own FACTORS, before the base pack's maps were multiplied into them
    // (`pbr_input.material` already carries that product). Every layer is multiplied by the same
    // pair, so the four packs answer to one material.
    let base_color_factor = pbr_bindings::material.base_color.rgb;
    let roughness_factor = pbr_bindings::material.perceptual_roughness;

    let ddx_uv = dpdx(in.uv);
    let ddy_uv = dpdy(in.uv);
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
        let uv = in.uv * scale;
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
    // The packs' print responses, by the same weights.
    var print_depth = 0.0;
    var print_albedo = 0.0;
    var print_roughness = 0.0;
    for (var i = 0u; i < SLOTS; i++) {
        let weight = weights[i] / total;
        blended_albedo += albedo[i] * weight;
        // Normals blend as VECTORS, in world space, and are renormalized below.
        blended_normal += normal[i] * weight;
        blended_occlusion += occlusion[i] * weight;
        blended_roughness += roughness[i] * weight;
        print_depth += blend.print_depth[i] * weight;
        print_albedo += blend.print_albedo[i] * weight;
        print_roughness += blend.print_roughness[i] * weight;
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

    // TRACK PRINTS. Far from the eye the world layer's coverage is all there is; inside the near
    // store its sharp footprint takes over, and the depth's gradient tilts the normal.
    let near = near_weight(world_xz, ddx_xz, ddy_xz);
    var footprint = trodden;
    var depth_slope = vec3(0.0);
    if near > 0.0 {
        let uv = world_xz / blend.print_near.z;
        let texel_uv = blend.print_near.w / blend.print_near.z;
        let here = textureSampleLevel(print_near_texture, layer_albedo_sampler, uv, 0.0).rg;
        let east = textureSampleLevel(
            print_near_texture, layer_albedo_sampler, uv + vec2(texel_uv, 0.0), 0.0).r;
        let west = textureSampleLevel(
            print_near_texture, layer_albedo_sampler, uv - vec2(texel_uv, 0.0), 0.0).r;
        let north = textureSampleLevel(
            print_near_texture, layer_albedo_sampler, uv + vec2(0.0, texel_uv), 0.0).r;
        let south = textureSampleLevel(
            print_near_texture, layer_albedo_sampler, uv - vec2(0.0, texel_uv), 0.0).r;
        // d(depth)/dx and d(depth)/dz, world space: the surface DESCENDS toward a deeper texel.
        let gradient = vec2(
            sunk(east, print_depth) - sunk(west, print_depth),
            sunk(north, print_depth) - sunk(south, print_depth),
        ) / (2.0 * blend.print_near.w);
        depth_slope = vec3(gradient.x, 0.0, gradient.y) * near;
        footprint = mix(trodden, here.g, near);
    }
    blended_albedo *= mix(1.0, print_albedo, footprint);
    blended_roughness *= mix(1.0, print_roughness, footprint);
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
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
