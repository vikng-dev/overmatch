// The scatter buildings' surface (`src/building_surface.rs` — `BuildingMaterial`): the wall pack
// through bevy's standard PBR path on every face, the roof pack swapped in on the timber.
//
// THE VERTEX STAGE PUTS EVERY FACE IN METRES. The mesh (`building_surface::unit_box`) is the unit
// cube each building's block transform scales, its UV in face units: `uv = (p·t, −p·b)` for a
// corner `p`, tangent `t` and image-up `b = w · n × t`. Under the instance's transform a face unit
// along `t` spans `|M t|` metres and one along `b` spans `|M b|`, so scaling the two UV lanes by
// those lengths maps every face at one UV per metre, whatever the box's proportions and the
// instance's scale. The packs' own repeats are applied after: the wall's by the standard half's
// `uv_transform`, the roof's by `surface.roof.x`.
//
// Otherwise bevy's own mesh vertex stage (`bevy_pbr/src/render/mesh.wgsl`) for an unskinned,
// unmorphed mesh, which a building always is.
//
// DERIVATIVES ARE TAKEN ONCE, UP FRONT. The roof branch is not uniform across a quad at the eaves,
// so its fetches are `textureSampleGrad` with the UV's gradients computed outside the branch.

#import bevy_pbr::{
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    mesh_functions,
    view_transformations::position_world_to_clip,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_bindings,
    pbr_functions,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types,
}

struct BuildingSurfaceParams {
    // x: the roof pack's repeats per metre. yzw: unused.
    roof: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> surface: BuildingSurfaceParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var roof_albedo_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var roof_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var roof_normal_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var roof_arm_texture: texture_2d<f32>;

// On a mesh with no second UV (the graybox box), a face is roof where its normal is within 60° of
// up: the box's top, on any building that stands within 60° of upright, and never a wall. A mesh
// built from a building's elements (`src/buildings/view.rs`) says it per vertex instead: the second
// UV's `x` is 1 on timber, 0 on masonry.
const ROOF_MIN_UP: f32 = 0.5;

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal, vertex.instance_index);
    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local, vec4(vertex.position, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);

    let axes = mat3x3(world_from_local[0].xyz, world_from_local[1].xyz, world_from_local[2].xyz);
    let image_up = vertex.tangent.w * cross(vertex.normal, vertex.tangent.xyz);
    out.uv = vertex.uv * vec2(length(axes * vertex.tangent.xyz), length(axes * image_up));
#ifdef VERTEX_UVS_B
    // The pack mark (`ROOF_MIN_UP`'s note), passed through unscaled.
    out.uv_b = vertex.uv_b;
#endif
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local, vertex.tangent, vertex.instance_index);

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index, world_from_local[3]);
#endif
    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    let roof_uv = in.uv * surface.roof.x;
    let ddx = dpdx(roof_uv);
    let ddy = dpdy(roof_uv);

    // The wall pack, through the whole standard path: albedo, normal map against the mesh
    // tangents, ARM, and every non-texture material parameter.
    var pbr_input = pbr_input_from_standard_material(in, is_front);

#ifdef VERTEX_UVS_B
    let roof = in.uv_b.x > 0.5;
#else
    let roof = in.world_normal.y > ROOF_MIN_UP;
#endif
    if roof {
        let double_sided = (pbr_input.material.flags
            & pbr_types::STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT) != 0u;
        let TBN = pbr_functions::calculate_tbn_mikktspace(pbr_input.world_normal, in.world_tangent);
        let albedo = textureSampleGrad(roof_albedo_texture, roof_sampler, roof_uv, ddx, ddy).rgb;
        let tangent_normal =
            textureSampleGrad(roof_normal_texture, roof_sampler, roof_uv, ddx, ddy).rgb;
        let arm = textureSampleGrad(roof_arm_texture, roof_sampler, roof_uv, ddx, ddy).rgb;
        // The material's own factors multiply the roof pack as they do the wall pack, so the two
        // answer to one material.
        pbr_input.material.base_color = vec4(
            pbr_bindings::material.base_color.rgb * albedo, pbr_input.material.base_color.a);
        pbr_input.N = pbr_functions::apply_normal_mapping(
            pbr_input.material.flags, TBN, double_sided, is_front, tangent_normal);
        pbr_input.diffuse_occlusion = vec3(arm.r);
        pbr_input.material.perceptual_roughness = pbr_bindings::material.perceptual_roughness * arm.g;
        // Metallic is not swapped: the material's metallic factor is hard zero, so either pack's
        // B channel multiplies out to a dielectric.
    }

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
