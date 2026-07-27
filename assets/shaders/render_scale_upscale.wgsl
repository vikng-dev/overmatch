// Render-scale upscale (src/render_scale.rs — `render_scale_upscale`).
//
// One fullscreen triangle that stretches the top-left sub-rect of the view's main texture — the
// region the 3D main pass actually rendered into under `MainPassResolutionOverride` — across the
// whole texture, bilinearly. It runs in `Core3dSystems::EarlyPostProcess`, so everything after it
// (bloom, tonemapping, the UI pass) sees a full-resolution image and needs no knowledge that the
// world was drawn smaller. That is Apple's own prescribed shape: render 3D into a smaller target,
// upscale it, composite native-resolution 2D on top.
//
// The two sizes are read from the GPU rather than passed in, so the shader cannot disagree with
// what the main pass did:
//   * `textureDimensions(in_texture)` — the FULL physical target size (the main textures are always
//     allocated at full size; the resolution override scales the pass, not the allocation).
//   * `view.main_pass_viewport.zw` — the size the main pass was scissored to, which is exactly
//     where bevy feeds `MainPassResolutionOverride` (bevy_render `prepare_view_uniforms`).
// At scale 1.0 no override exists, the two are equal, and this pass is not scheduled at all.
//
// The half-texel clamp is load-bearing. The naive mapping `uv * rendered / full` lands the last
// destination column exactly on the boundary texel, where a FILTERING sampler blends in the
// never-written garbage beyond the rendered region — a bright or black seam down the right and
// bottom edges. Clamping the source coordinate to the [0.5, rendered - 0.5] texel-centre band keeps
// every tap inside what the main pass drew.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import bevy_render::view::View

@group(0) @binding(0) var in_texture: texture_2d<f32>;
@group(0) @binding(1) var in_sampler: sampler;
@group(0) @binding(2) var<uniform> view: View;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let full = vec2<f32>(textureDimensions(in_texture));
    let rendered = view.main_pass_viewport.zw;
    // Destination pixel centre -> source texel coordinate, clamped to the rendered region's texel
    // centres (see the header). `max(..., 0.5)` guards a degenerate 1-texel render target.
    let hi = max(rendered - vec2<f32>(0.5), vec2<f32>(0.5));
    let texel = clamp(in.uv * rendered, vec2<f32>(0.5), hi);
    return textureSample(in_texture, in_sampler, texel / full);
}
