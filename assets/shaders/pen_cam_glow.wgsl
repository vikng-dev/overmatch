// The pen cam's stage material (`src/damage_cam/pen_cam/material.rs`, `GlowMaterial`): one flat
// colour and an optional view-angle rim. Unlit: everything it draws is a diagram over the record.
//
// Output: `rgb` with alpha 0 into the premultiplied pipeline `AlphaMode::Add` selects, so overlap
// order never changes the sum.

#import bevy_pbr::{forward_io::Vertex, mesh_functions, mesh_view_bindings::view}

struct GlowParams {
    // rgb: linear colour; a: overall fade.
    color: vec4<f32>,
    // x: base intensity, y: rim gain, z: rim power.
    rim: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> glow: GlowParams;

struct GlowVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
}

@vertex
fn vertex(vertex: Vertex) -> GlowVertexOutput {
    var out: GlowVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(vertex.position, 1.0));
    out.position = view.clip_from_world * vec4(out.world_position.xyz, 1.0);
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#else
    out.world_normal = vec3(0.0, 1.0, 0.0);
#endif
    return out;
}

@fragment
fn fragment(in: GlowVertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let v = normalize(view.world_position - in.world_position.xyz);
    let facing = abs(dot(n, v));
    let intensity = glow.rim.x + glow.rim.y * pow(1.0 - facing, glow.rim.z);
    return vec4(glow.color.rgb * intensity * glow.color.a, 0.0);
}
