// The look's vertex stage for a live track belt (`look::BeltLook`): `pen_cam_look.wesl`'s `vertex`
// with each shoe placed in the belt's frame first (`track_belt.wesl`), so the look and its masks
// draw every shoe where the belt's own material does. The output is the look's
// `LookVertexOutput`, which `pen_cam_look.wesl`'s fragment stage reads by location.

import bevy_pbr::render::{forward_io::{Vertex, decompress_vertex}, mesh_functions, mesh_view_bindings::view};
import super::track_belt::{belt_at, link_position, link_direction};

struct LookVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
}

@vertex
fn vertex(vertex_in: Vertex, @location(15) link: u32) -> LookVertexOutput {
    let vertex = decompress_vertex(vertex_in, vertex_in.instance_index);
    var out: LookVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let belt = belt_at(vertex.instance_index);
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(link_position(belt, link, vertex.position), 1.0));
    out.position = view.clip_from_world * vec4(out.world_position.xyz, 1.0);
@if(VERTEX_NORMALS) {
    out.world_normal = mesh_functions::mesh_normal_local_to_world(link_direction(belt, link, vertex.normal), vertex.instance_index);
}
@else {
    out.world_normal = vec3(0.0, 1.0, 0.0);
}
    return out;
}
