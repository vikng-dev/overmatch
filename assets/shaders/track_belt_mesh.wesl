// The belt's vertex stage for its `StandardMaterial` look (`track::belt_draw::BeltShoes`), in the
// main pass and in the prepass (depth, and shadow casting) alike: `PREPASS_PIPELINE` selects the
// matching IO. It is bevy's own mesh vertex stage (`bevy_pbr` 0.19 `mesh.wesl` / `prepass.wesl`)
// with the shoe placed in the belt's frame first (`track_belt.wesl`) and no skinning or morph
// targets, which a belt mesh never carries. Main and prepass place a vertex through the one
// function, so the depth they write agrees.

import bevy_pbr::render::{mesh_functions, view_transformations::position_world_to_clip};
import super::track_belt::{belt_at, link_position, link_direction};

@if(PREPASS_PIPELINE)
import bevy_pbr::prepass::io::{Vertex, VertexOutput, decompress_vertex};
@else
import bevy_pbr::render::forward_io::{Vertex, VertexOutput, decompress_vertex};

@vertex
fn vertex(vertex_in: Vertex, @location(15) link: u32) -> VertexOutput {
    let vertex = decompress_vertex(vertex_in, vertex_in.instance_index);
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let belt = belt_at(vertex.instance_index);
    let local = link_position(belt, link, vertex.position);
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(local, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);

@if(PREPASS_PIPELINE) {
@if(UNCLIPPED_DEPTH_ORTHO_EMULATION) {
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
}
}

@if(VERTEX_UVS_A) {
    out.uv = vertex.uv;
}
@if(VERTEX_UVS_B) {
    out.uv_b = vertex.uv_b;
}

@if(PREPASS_PIPELINE) {
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS) {
@if(VERTEX_NORMALS) {
    out.world_normal = mesh_functions::mesh_normal_local_to_world(link_direction(belt, link, vertex.normal), vertex.instance_index);
}
@if(VERTEX_TANGENTS) {
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vec4<f32>(link_direction(belt, link, vertex.tangent.xyz), vertex.tangent.w),
        vertex.instance_index
    );
}
}
}
@else {
@if(VERTEX_NORMALS) {
    out.world_normal = mesh_functions::mesh_normal_local_to_world(link_direction(belt, link, vertex.normal), vertex.instance_index);
}
@if(VERTEX_TANGENTS) {
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vec4<f32>(link_direction(belt, link, vertex.tangent.xyz), vertex.tangent.w),
        vertex.instance_index
    );
}
}

@if(VERTEX_COLORS) {
    out.color = vertex.color;
}

@if(PREPASS_PIPELINE) {
@if(MOTION_VECTOR_PREPASS) {
    // The belt's previous transform carrying this frame's span: a link's own motion along the
    // belt is not a motion vector (no table history is kept, and no pass reads one).
    out.previous_world_position = mesh_functions::mesh_position_local_to_world(
        mesh_functions::get_previous_world_from_local(vertex.instance_index),
        vec4<f32>(local, 1.0)
    );
}
}

@if(VERTEX_OUTPUT_INSTANCE_INDEX) {
    out.instance_index = vertex.instance_index;
}

@if(VISIBILITY_RANGE_DITHER) {
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index, world_from_local[3]);
}

    return out;
}
