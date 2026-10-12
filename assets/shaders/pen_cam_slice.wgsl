// The pen cam's plate slice (`src/damage_cam/pen_cam/material.rs`, `SliceCut`): a standard PBR
// fragment with the eye's side of a plane removed. Removal is a screen-door dissolve: a fragment on
// the eye's side is discarded when its ordered-dither threshold is below `params.x` (0 keeps all of
// it, 1 removes all of it). A back face — the inside of a closed armour volume, seen through the cut
// — is the flat cut colour under a 45° section hatch.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}

struct SliceParams {
    // xyz: unit plane normal toward the eye, w: normal . point.
    plane: vec4<f32>,
    cut_color: vec4<f32>,
    // x: removal 0..1, y: hatch period (physical px), z: hatch contrast 0..1.
    params: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> slice: SliceParams;

// The 4x4 Bayer threshold at a pixel, in (0, 1).
fn bayer(p: vec2<f32>) -> f32 {
    let x = u32(p.x) % 4u;
    let y = u32(p.y) % 4u;
    var m = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    return (m[y * 4u + x] + 0.5) / 16.0;
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    if dot(slice.plane.xyz, in.world_position.xyz) > slice.plane.w
        && bayer(in.position.xy) < slice.params.x {
        discard;
    }
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var out: FragmentOutput;
    if !is_front {
        let stripe = fract((in.position.x + in.position.y) / slice.params.y);
        var cut = slice.cut_color.rgb;
        if stripe < 0.5 {
            cut = cut * (1.0 - slice.params.z);
        }
        out.color = main_pass_post_lighting_processing(pbr_input, vec4(cut, 1.0));
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
