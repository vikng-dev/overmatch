// The damage cam's section cut: a fragment on the eye's side of the plane is discarded, a back face
// (the inside of a watertight armour volume, seen through the cut) is painted the flat cut colour,
// and everything else is bevy's standard PBR fragment. The CPU half, which also owns the plane
// convention, is `src/damage_cam/player.rs` (`SectionCutParams`).

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}

struct SectionCutParams {
    // xyz: unit plane normal toward the eye, w: normal . point.
    plane: vec4<f32>,
    cut_color: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> section: SectionCutParams;

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    if dot(section.plane.xyz, in.world_position.xyz) > section.plane.w {
        discard;
    }
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var out: FragmentOutput;
    if !is_front {
        out.color = main_pass_post_lighting_processing(pbr_input, section.cut_color);
        return out;
    }
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
