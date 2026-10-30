// The look's vertex stage for a live track belt (`look::BeltLook`): `pen_cam_look.wgsl`'s `vertex`
// with each shoe placed in the belt's frame first (`track_belt.wgsl`), so the look and its masks
// draw every shoe where the belt's own material does. The output is the look's
// `LookVertexOutput`, which `pen_cam_look.wgsl`'s fragment stage reads by location.

#import bevy_pbr::{forward_io::Vertex, mesh_functions, mesh_view_bindings::view}
#import "shaders/track_belt.wgsl"::{link_position, link_direction}

struct LookVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
}

@vertex
fn vertex(vertex: Vertex, @location(15) link: u32) -> LookVertexOutput {
    var out: LookVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(link_position(link, vertex.position), 1.0));
    out.position = view.clip_from_world * vec4(out.world_position.xyz, 1.0);
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(link_direction(link, vertex.normal), vertex.instance_index);
#else
    out.world_normal = vec3(0.0, 1.0, 0.0);
#endif
    return out;
}
