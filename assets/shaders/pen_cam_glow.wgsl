// The pen cam's stage material (`src/damage_cam/pen_cam/material.rs`, `GlowMaterial`): one flat
// colour, an optional view-angle rim or edge colour, optional vertex colour, an optional clip plane,
// an optional 45° section hatch, an optional 1/r² falloff from the mesh's origin and an optional
// front that holds every vertex within a radius of that origin. Unlit: everything it draws is a
// diagram over the record.
//
// Output: additive (`flags.z = 1`) writes `rgb` with alpha 0 into the premultiplied pipeline
// `AlphaMode::Add` selects, so overlap order never changes the sum; blended (`flags.z = 0`) writes
// straight `rgb, a` for `AlphaMode::Blend`.

#import bevy_pbr::{forward_io::Vertex, mesh_functions, mesh_view_bindings::view}

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
    // rgb: the edge colour the rim term mixes the fill toward; a: edge on (1) or off (0).
    edge: vec4<f32>,
    // x: front radius (m, world) from the mesh's origin, negative for none; y: falloff reference
    // radius r0 (m), 0 for none.
    field: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> glow: GlowParams;

struct GlowVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    // Distance (m, world) from the mesh's origin.
    @location(3) radius: f32,
}

@vertex
fn vertex(vertex: Vertex) -> GlowVertexOutput {
    var out: GlowVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var local = vertex.position;
    if glow.field.x >= 0.0 {
        let reach = length((world_from_local * vec4(local, 0.0)).xyz);
        if reach > glow.field.x {
            local = local * (glow.field.x / reach);
        }
    }
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(local, 1.0));
    out.position = view.clip_from_world * vec4(out.world_position.xyz, 1.0);
    out.radius = length(out.world_position.xyz - (world_from_local * vec4(0.0, 0.0, 0.0, 1.0)).xyz);
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#else
    out.world_normal = vec3(0.0, 1.0, 0.0);
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#else
    out.color = vec4(1.0);
#endif
    return out;
}

@fragment
fn fragment(
    in: GlowVertexOutput,
    @builtin(front_facing) is_front: bool,
) -> @location(0) vec4<f32> {
    if glow.flags.x > 0.5 && dot(glow.plane.xyz, in.world_position.xyz) > glow.plane.w {
        discard;
    }
    let n = normalize(in.world_normal);
    let v = normalize(view.world_position - in.world_position.xyz);
    let facing = abs(dot(n, v));
    let rim = glow.rim.y * pow(1.0 - facing, glow.rim.z);
    var intensity = glow.rim.x + rim;
    var rgb = glow.color.rgb;
    rgb = mix(rgb, rgb * in.color.rgb, glow.rim.w);
    intensity = mix(intensity, intensity * in.color.a, glow.rim.w);
    if glow.field.y > 0.0 {
        let r = max(in.radius, 1e-4);
        intensity = intensity * min(1.0, (glow.field.y / r) * (glow.field.y / r));
    }
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
    if glow.edge.a > 0.5 {
        let e = clamp(rim, 0.0, 1.0);
        return vec4(mix(rgb * glow.rim.x, glow.edge.rgb, e), mix(alpha, 1.0, e) * fade);
    }
    return vec4(rgb * intensity, alpha * fade);
}
