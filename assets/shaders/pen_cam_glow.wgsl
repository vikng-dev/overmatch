// The pen cam's stage material (`src/damage_cam/pen_cam/material.rs`, `GlowMaterial`): one flat
// colour, an optional view-angle rim, optional vertex colour, an optional clip plane and an optional
// 45° section hatch. Unlit: everything it draws is a diagram over the record.
//
// Output: additive (`flags.z = 1`) writes `rgb` with alpha 0 into the premultiplied pipeline
// `AlphaMode::Add` selects, so overlap order never changes the sum; blended (`flags.z = 0`) writes
// straight `rgb, a` for `AlphaMode::Blend`.

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::view

struct GlowParams {
    // rgb: linear colour; a: opacity for the blended output.
    color: vec4<f32>,
    // x: base intensity, y: rim gain, z: rim power, w: vertex colour weight (0 or 1).
    rim: vec4<f32>,
    // xyz: unit plane normal toward the discarded side, w: normal . point.
    plane: vec4<f32>,
    // x: clip on (1) or off (0), y: hatch on (1) or off (0), z: additive (1) or blended (0),
    // w: overall fade.
    flags: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> glow: GlowParams;

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> @location(0) vec4<f32> {
    if glow.flags.x > 0.5 && dot(glow.plane.xyz, in.world_position.xyz) > glow.plane.w {
        discard;
    }
    let n = normalize(in.world_normal);
    let v = normalize(view.world_position - in.world_position.xyz);
    let facing = abs(dot(n, v));
    var intensity = glow.rim.x + glow.rim.y * pow(1.0 - facing, glow.rim.z);
    var rgb = glow.color.rgb;
#ifdef VERTEX_COLORS
    rgb = mix(rgb, rgb * in.color.rgb, glow.rim.w);
    intensity = mix(intensity, intensity * in.color.a, glow.rim.w);
#endif
    var alpha = glow.color.a;
    if glow.flags.y > 0.5 {
        // 45° lines of #8A96A3 (linear below), 6 physical pixels apart, a quarter of each period.
        let stripe = fract((in.position.x + in.position.y) / 6.0);
        if stripe < 0.25 {
            rgb = vec3(0.254, 0.305, 0.366);
        }
    }
    let fade = glow.flags.w;
    if glow.flags.z > 0.5 {
        return vec4(rgb * intensity * fade, 0.0);
    }
    return vec4(rgb * intensity, alpha * fade);
}
