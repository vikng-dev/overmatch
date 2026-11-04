// The scatter constructions' surface (`src/construction_surface.rs` — `ConstructionMaterial`): the primary pack
// through bevy's standard PBR path on every face, the secondary pack swapped in on the timber.
//
// THE VERTEX STAGE PUTS EVERY FACE IN METRES. The mesh (`construction_surface::unit_box`) is the unit
// cube each construction's block transform scales, its UV in face units: `uv = (p·t, −p·b)` for a
// corner `p`, tangent `t` and image-up `b = w · n × t`. Under the instance's transform a face unit
// along `t` spans `|M t|` metres and one along `b` spans `|M b|`, so scaling the two UV lanes by
// those lengths maps every face at one UV per metre, whatever the box's proportions and the
// instance's scale. The packs' own repeats are applied after: the primary's by the standard half's
// `uv_transform`, the secondary's by `surface.secondary.x`.
//
// Otherwise bevy's own mesh vertex stage (`bevy_pbr/src/render/mesh.wgsl`) for an unskinned,
// unmorphed mesh, which a construction always is.
//
// DERIVATIVES ARE TAKEN ONCE, UP FRONT. The secondary branch is not uniform across a quad at the eaves,
// so its fetches are `textureSampleGrad` with the UV's gradients computed outside the branch.
//
// HOLES ONLY WHERE THE MATERIAL MAY DISCARD. A construction with a hole wears its own copy of the
// material, alpha-masked (`construction::view`), and only that variant compiles the hole path
// (`construction_holes.wgsl`): a construction without one draws exactly as it would with no hole code.

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
#ifdef MAY_DISCARD
#import "shaders/construction_holes.wgsl"::{hole_at, SEE_THROUGH, INNER_WALL}
#endif

struct SurfaceParams {
    // x: the secondary pack's repeats per metre. yzw: unused.
    secondary: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> surface: SurfaceParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var secondary_albedo_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var secondary_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var secondary_normal_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var secondary_arm_texture: texture_2d<f32>;

// Every construction mesh supplies UV1: x is the declared palette lane, y its hole tag.

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
    out.uv_b = vertex.uv_b;
#ifdef VERTEX_COLORS
    // A dressing's tint (`construction::view`): the standard path multiplies the primary pack by it.
    out.color = vertex.color;
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
    let secondary_uv = in.uv * surface.secondary.x;
    let ddx = dpdx(secondary_uv);
    let ddy = dpdy(secondary_uv);

    // The primary pack, through the whole standard path: albedo, normal map against the mesh
    // tangents, ARM, and every non-texture material parameter.
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let secondary = in.uv_b.x > 0.5;
    if secondary {
        let double_sided = (pbr_input.material.flags
            & pbr_types::STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT) != 0u;
        let TBN = pbr_functions::calculate_tbn_mikktspace(pbr_input.world_normal, in.world_tangent);
        let albedo = textureSampleGrad(secondary_albedo_texture, secondary_sampler, secondary_uv, ddx, ddy).rgb;
        let tangent_normal =
            textureSampleGrad(secondary_normal_texture, secondary_sampler, secondary_uv, ddx, ddy).rgb;
        let arm = textureSampleGrad(secondary_arm_texture, secondary_sampler, secondary_uv, ddx, ddy).rgb;
        // The material's own factors multiply the secondary pack as they do the primary pack, so the two
        // answer to one material.
        var tint = vec3(1.0);
#ifdef VERTEX_COLORS
        tint = in.color.rgb;
#endif
        pbr_input.material.base_color = vec4(
            pbr_bindings::material.base_color.rgb * albedo * tint, pbr_input.material.base_color.a);
        pbr_input.N = pbr_functions::apply_normal_mapping(
            pbr_input.material.flags, TBN, double_sided, is_front, tangent_normal);
        pbr_input.diffuse_occlusion = vec3(arm.r);
        pbr_input.material.perceptual_roughness = pbr_bindings::material.perceptual_roughness * arm.g;
        pbr_input.material.metallic = pbr_bindings::material.metallic * arm.b;
    }

#ifdef MAY_DISCARD
#ifdef VERTEX_UVS_B
    // Through a hole the eye sees past the chunk, or the hole's inner wall: the face's own pack,
    // lit where the wall is and facing into the hole, without the face's normal map.
    let hole = hole_at(in.world_position.xyz, in.uv_b.y, in.instance_index);
    if hole.kind == SEE_THROUGH {
        discard;
    }
    if hole.kind == INNER_WALL {
        pbr_input.world_position = vec4(hole.at, 1.0);
        pbr_input.world_normal = hole.normal;
        pbr_input.N = hole.normal;
        pbr_input.V = pbr_functions::calculate_view(
            pbr_input.world_position, pbr_input.is_orthographic);
    }
#endif
#endif

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
