// A `StandardMaterial` look that receives the terrain shadow (`src/terrain_shadow.rs`): bevy's own
// forward fragment (`bevy_pbr`'s `render/pbr.wesl`, its forward branch, forward decals included)
// with the field's visibility multiplied into the sun's shadow. It draws the track belt
// (`BeltShoes`) and the hull marks (`TerrainShadowed` under bevy's forward decal). The forward-only
// game runs no meshlet, deferred or OIT pass, so their branches are not carried.

import bevy_pbr::{
    render::{
        forward_io::{VertexOutput, FragmentOutput},
        pbr_fragment::pbr_input_from_standard_material,
        pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
        pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    },
    decal::clustered::apply_decals,
};

@if(VISIBILITY_RANGE_DITHER)
import bevy_pbr::render::pbr_functions::visibility_range_dither;

@if(FORWARD_DECAL)
import bevy_pbr::decal::forward::get_forward_decal_info;

import super::terrain_shadow::visibility;

@fragment
fn fragment(vertex_output: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var in = vertex_output;

@if(VISIBILITY_RANGE_DITHER)
    visibility_range_dither(in.position, in.visibility_range_dither);

@if(FORWARD_DECAL)
    let forward_decal_info = get_forward_decal_info(in);
@if(FORWARD_DECAL)
    in.world_position = forward_decal_info.world_position;
@if(FORWARD_DECAL)
    in.uv = forward_decal_info.uv;

    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color
    );
    apply_decals(&pbr_input);
    pbr_input.directional_shadow_factor *= visibility(pbr_input.world_position.xyz);

    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);

@if(FORWARD_DECAL)
    out.color.a = min(forward_decal_info.alpha, out.color.a);

    return out;
}
