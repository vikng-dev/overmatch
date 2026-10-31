// The tank surface (`src/tank_surface.rs`, `TankSurfaceMaterial`): a material's visual, then the
// livery's paint and camouflage where the visual takes paint, through bevy's standard PBR lighting.
// Design: `.agents/docs/design/tank-surfaces.md`.
//
// TRIPLANAR IN THE REST FRAME, NEVER THROUGH UVS. A position is projected from the fragment's
// mesh-local position carried into its part's REST FRAME by the primitive's row of `rest_frames`
// (the row its `MeshTag` names): the mesh-local frame through every node up to, and not including,
// the first one the tank file moves. Plates of one rigid part share that frame, so a pattern runs on
// across them; a moving part carries the frame with it, so nothing swims. The row is rigid (rotation
// and translation): the rest frame is in metres.
//
// Each projection axis reads a map at `uv = (a, −b)` for its two in-plane axes `(a, b)` — X: (z, y),
// Y: (x, z), Z: (x, y) — so `+a` is the image's right and `+b` its top, the direction an OpenGL
// (`nor_gl`) normal map's green points. Normals blend by the whiteout rule in the rest frame and go to
// world space through the mesh's inverse transpose.
//
// DERIVATIVES ARE TAKEN ONCE, UP FRONT. A projection whose weight is below `SKIP` is not sampled, a
// branch that is not uniform across a quad, so every fetch is `textureSampleGrad` with gradients
// computed outside it.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_functions,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
}

struct TankSurfaceParams {
    // rgb: the livery's paint, linear. a: its perceptual roughness (1 − sheen).
    paint: vec4<f32>,
    // The livery's camouflage colours, linear rgb, for the pattern's R, G and B coverage in turn.
    // a: 1 where the livery names that colour, 0 where it does not.
    camo_r: vec4<f32>,
    camo_g: vec4<f32>,
    camo_b: vec4<f32>,
    // x: the visual's repeats per metre. y: the camouflage pattern's repeats per metre. zw: unused.
    scale: vec4<f32>,
    // `FLAG_*` below.
    flags: u32,
}

// One rest frame: the rows of the rigid 3×4 transform from a primitive's mesh-local frame to its
// part's rest frame.
struct RestFrame {
    x: vec4<f32>,
    y: vec4<f32>,
    z: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> surface: TankSurfaceParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var visual_albedo: texture_2d<f32>;
// The one sampler of the extension (repeat, trilinear, anisotropic): every map and the pattern read
// through it. Bevy takes it from the image at 101, which a finished surface always binds: the
// visual's albedo, or a stand-in carrying the same sampler where the surface draws none
// (`src/tank_surface.rs`). A surface waiting for its maps binds nothing there, so this is bevy's
// clamping fallback, but its flags are 0 and nothing below samples through it. Every sampler a
// material declares counts against the 16 a stage gets (MEASURED: wgpu's default
// `max_samplers_per_shader_stage`, wgpu-types 29 `limits.rs`).
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var surface_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var visual_normal: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var visual_arm: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var camo_pattern: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var<storage, read> rest_frames: array<RestFrame>;

// The visual ships maps (`Maps` in `visuals.ron`); without, the standard half's factors are its look.
const FLAG_MAPS: u32 = 1u;
// The visual takes the livery's paint.
const FLAG_PAINT: u32 = 2u;
// The livery carries a camouflage pattern.
const FLAG_CAMO: u32 = 4u;

// How sharply the projections hand over around an edge: the weight of an axis is the eighth power
// of the normal's component along it. ASSUMED: a fillet of a few degrees blends, a flat plate reads
// one projection.
// A projection weighing less than this is skipped and the rest renormalised. ASSUMED.
const SKIP: f32 = 0.02;

// The in-plane axes `(a, −b)` of projection `axis` (0 X, 1 Y, 2 Z) of a rest-frame position or of
// one of its screen derivatives.
fn project(p: vec3<f32>, axis: u32) -> vec2<f32> {
    switch axis {
        case 0u: { return vec2(p.z, -p.y); }
        case 1u: { return vec2(p.x, -p.z); }
        default: { return vec2(p.x, -p.y); }
    }
}

// The whiteout blend of tangent-space normal `t` (read on projection `axis`) onto the rest-frame
// surface normal `n`: a rest-frame vector, unnormalised.
fn whiteout(t: vec3<f32>, n: vec3<f32>, axis: u32) -> vec3<f32> {
    switch axis {
        case 0u: {
            let b = vec3(t.xy + n.zy, abs(t.z) * n.x);
            return b.zyx;
        }
        case 1u: {
            let b = vec3(t.xy + n.xz, abs(t.z) * n.y);
            return b.xzy;
        }
        default: {
            return vec3(t.xy + n.xy, abs(t.z) * n.z);
        }
    }
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // The standard half: the material's factors (white and 1 under a pack, the visual's own on a flat
    // one), bevy's own normal preparation and every non-texture parameter.
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let world_from_local = mesh_functions::get_world_from_local(in.instance_index);
    let local_from_world = mesh_functions::get_local_from_world(in.instance_index);
    let row = rest_frames[mesh_functions::get_tag(in.instance_index)];
    let local = (local_from_world * in.world_position).xyz;
    let p = vec3(
        dot(row.x, vec4(local, 1.0)),
        dot(row.y, vec4(local, 1.0)),
        dot(row.z, vec4(local, 1.0)),
    );
    let dpx = dpdx(p);
    let dpy = dpdy(p);
    // Rest from local, rotation only (the rows are rigid), and the normal's way back to world:
    // local → world through the mesh's inverse transpose.
    let rest_from_local = transpose(mat3x3(row.x.xyz, row.y.xyz, row.z.xyz));
    let world_m3 = mat3x3(world_from_local[0].xyz, world_from_local[1].xyz, world_from_local[2].xyz);
    let local_m3 = mat3x3(local_from_world[0].xyz, local_from_world[1].xyz, local_from_world[2].xyz);
    let n = normalize(rest_from_local * (transpose(world_m3) * pbr_input.world_normal));

    let square = n * n;
    let fourth = square * square;
    var weights = fourth * fourth;
    weights /= weights.x + weights.y + weights.z;
    weights = select(weights, vec3(0.0), weights < vec3(SKIP));
    weights /= weights.x + weights.y + weights.z;

    let flags = surface.flags;
    let paint = (flags & FLAG_PAINT) != 0u;
    var albedo = pbr_input.material.base_color.rgb;
    var roughness = pbr_input.material.perceptual_roughness;
    var metallic = pbr_input.material.metallic;
    var occlusion = 1.0;
    var normal = n;

    if (flags & FLAG_MAPS) != 0u {
        let s = surface.scale.x;
        var sampled_albedo = vec3(0.0);
        var arm = vec3(0.0);
        var blended = vec3(0.0);
        for (var axis = 0u; axis < 3u; axis++) {
            let w = weights[axis];
            if w <= 0.0 {
                continue;
            }
            let uv = project(p, axis) * s;
            let ddx = project(dpx, axis) * s;
            let ddy = project(dpy, axis) * s;
            // The albedo is the bare visual's: paint covers it whole.
            if !paint {
                sampled_albedo += w * textureSampleGrad(visual_albedo, surface_sampler, uv, ddx, ddy).rgb;
            }
            arm += w * textureSampleGrad(visual_arm, surface_sampler, uv, ddx, ddy).rgb;
            let t = textureSampleGrad(visual_normal, surface_sampler, uv, ddx, ddy).rgb * 2.0 - 1.0;
            blended += w * whiteout(t, n, axis);
        }
        if !paint {
            albedo *= sampled_albedo;
        }
        occlusion = arm.r;
        roughness *= arm.g;
        metallic *= arm.b;
        normal = normalize(blended);
    }

    if paint {
        albedo = surface.paint.rgb;
        roughness = surface.paint.a;
        metallic = 0.0;
        if (flags & FLAG_CAMO) != 0u {
            let s = surface.scale.y;
            var coverage = vec3(0.0);
            for (var axis = 0u; axis < 3u; axis++) {
                let w = weights[axis];
                if w <= 0.0 {
                    continue;
                }
                let uv = project(p, axis) * s;
                coverage += w * textureSampleGrad(
                    camo_pattern, surface_sampler, uv, project(dpx, axis) * s,
                    project(dpy, axis) * s).rgb;
            }
            coverage *= vec3(surface.camo_r.a, surface.camo_g.a, surface.camo_b.a);
            // A pattern's coverage is non-negative and sums to at most 1 per texel
            // (`scripts/visuals/camo_stripes.py` cuts the second colour by the first), and linear
            // filtering and the projection blend keep that bound. Quantisation and compression can
            // push a sum just past 1, and a pattern made otherwise further: such a sum is scaled
            // back so the base paint's weight never goes negative.
            coverage /= max(1.0, coverage.r + coverage.g + coverage.b);
            albedo = albedo * (1.0 - coverage.r - coverage.g - coverage.b)
                + surface.camo_r.rgb * coverage.r
                + surface.camo_g.rgb * coverage.g
                + surface.camo_b.rgb * coverage.b;
        }
    }

    pbr_input.material.base_color = vec4(albedo, pbr_input.material.base_color.a);
    pbr_input.material.perceptual_roughness = roughness;
    pbr_input.material.metallic = metallic;
    pbr_input.diffuse_occlusion *= occlusion;
    pbr_input.N = normalize(transpose(local_m3) * (transpose(rest_from_local) * normal));

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
