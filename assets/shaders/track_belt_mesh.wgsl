// The belt's vertex stage for its `StandardMaterial` look (`track::belt_draw::BeltShoes`), in the
// main pass and in the prepass (depth, and shadow casting) alike: `PREPASS_PIPELINE` selects the
// matching IO. It is bevy's own mesh vertex stage (`bevy_pbr` 0.19 `mesh.wgsl` / `prepass.wgsl`)
// with the shoe placed in the belt's frame first (`track_belt.wgsl`) and no skinning or morph
// targets, which a belt mesh never carries. Main and prepass place a vertex through the one
// function, so the depth they write agrees.

#import bevy_pbr::{mesh_functions, view_transformations::position_world_to_clip}
#import "shaders/track_belt.wgsl"::{belt_at, link_position, link_direction}

#ifdef PREPASS_PIPELINE
#import bevy_pbr::prepass_io::{Vertex, VertexOutput}
#else
#import bevy_pbr::forward_io::{Vertex, VertexOutput}
#endif

@vertex
fn vertex(vertex: Vertex, @location(15) link: u32) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let belt = belt_at(vertex.instance_index);
    let local = link_position(belt, link, vertex.position);
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(local, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);

#ifdef PREPASS_PIPELINE
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif

#ifdef PREPASS_PIPELINE
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(link_direction(belt, link, vertex.normal), vertex.instance_index);
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vec4<f32>(link_direction(belt, link, vertex.tangent.xyz), vertex.tangent.w),
        vertex.instance_index
    );
#endif
#endif
#else
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(link_direction(belt, link, vertex.normal), vertex.instance_index);
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vec4<f32>(link_direction(belt, link, vertex.tangent.xyz), vertex.tangent.w),
        vertex.instance_index
    );
#endif
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef PREPASS_PIPELINE
#ifdef MOTION_VECTOR_PREPASS
    // The belt's previous transform carrying this frame's span: a link's own motion along the
    // belt is not a motion vector (no table history is kept, and no pass reads one).
    out.previous_world_position = mesh_functions::mesh_position_local_to_world(
        mesh_functions::get_previous_world_from_local(vertex.instance_index),
        vec4<f32>(local, 1.0)
    );
#endif
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif

#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index, world_from_local[3]);
#endif

    return out;
}
