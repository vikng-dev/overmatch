// The pen cam's component outline (`src/damage_cam/pen_cam/material.rs`, `OutlineMaterial`): an
// inverted hull. Each vertex is pushed out on screen, away from the projected centre, by a fixed
// width in physical pixels; the pipeline culls front faces, so the hull's back faces show only where
// they stand outside the opaque fill drawn by the mesh itself, as a band of that width around its
// silhouette. Pushing from one centre rather than along each vertex normal keeps a hard-edged
// (split-normal) mesh's hull closed at its corners.

#import bevy_pbr::{forward_io::Vertex, mesh_functions, mesh_view_bindings::view}

struct OutlineParams {
    // rgb: linear colour; a: fade (the band is discarded below one half).
    color: vec4<f32>,
    // xyz: the centre the band is pushed out from, in the mesh's own frame; w: width (physical px).
    center: vec4<f32>,
    // x: the split corner's side it draws on: 0 both, 1 the record's camera, 2 the comparison's,
    // whose viewport starts right of the image's origin.
    side: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> outline: OutlineParams;

@vertex
fn vertex(vertex: Vertex) -> @builtin(position) vec4<f32> {
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let world = world_from_local * vec4(vertex.position, 1.0);
    let center = world_from_local * vec4(outline.center.xyz, 1.0);
    var clip = view.clip_from_world * world;
    let middle = view.clip_from_world * center;
    let away = clip.xy / clip.w - middle.xy / middle.w;
    let length_ndc = length(away);
    if length_ndc > 1e-6 && clip.w > 0.0 {
        // One physical pixel is 2 / viewport size in NDC.
        let push = (away / length_ndc) * outline.center.w * 2.0 / view.viewport.zw;
        clip = vec4(clip.xy + push * clip.w, clip.zw);
    }
    return clip;
}

@fragment
fn fragment() -> @location(0) vec4<f32> {
    let comparison = view.viewport.x > 0.5;
    let side = i32(outline.side.x + 0.5);
    if outline.color.a < 0.5 || (side == 1 && comparison) || (side == 2 && !comparison) {
        discard;
    }
    return vec4(outline.color.rgb, 1.0);
}
