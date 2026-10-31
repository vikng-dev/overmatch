// The buildings' prepass fragment (`src/building_surface.rs`), which bevy runs only for a material
// that may discard: a building with a hole (`building_holes.wgsl`). The shadow pass is a prepass, so
// a light passes through a hole wherever its ray leaves the chunk through it, and an inner wall
// casts where the face it is seen through would.
//
// No game camera carries a normal, motion-vector or deferred prepass (`scene_depth`), so the
// normal written here is the face's own, without the normal map bevy's `pbr_prepass.wgsl` applies.

#import bevy_pbr::{prepass_io::VertexOutput, pbr_functions, pbr_prepass_functions}
#import "shaders/building_holes.wgsl"::{hole_at, SEE_THROUGH}

#ifdef PREPASS_FRAGMENT
#import bevy_pbr::prepass_io::FragmentOutput
#endif

// Skip a fragment the view's ray sees through a hole.
fn see_through(in: VertexOutput) {
#ifdef VERTEX_UVS_B
    if hole_at(in.world_position.xyz, in.uv_b.y, in.instance_index).kind == SEE_THROUGH {
        discard;
    }
#endif
}

#ifdef PREPASS_FRAGMENT
@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    see_through(in);
    var out: FragmentOutput;
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.frag_depth = in.unclipped_depth;
#endif
#ifdef NORMAL_PREPASS
    let world_normal = pbr_functions::prepare_world_normal(in.world_normal, false, is_front);
    out.normal = vec4(world_normal * 0.5 + vec3(0.5), 1.0);
#endif
#ifdef MOTION_VECTOR_PREPASS
    out.motion_vector = pbr_prepass_functions::calculate_motion_vector(
        in.world_position, in.previous_world_position);
#endif
    return out;
}
#else
@fragment
fn fragment(in: VertexOutput) {
    see_through(in);
}
#endif
