// Flipbook billboard fragment (src/vfx/billboard.rs — `VfxBillboardMaterial`).
//
// One grayscale atlas + a per-effect gradient LUT, three craft tricks in one pass:
//   * flipbook cell select — the CPU picks the (wrapped, random-start-offset) frame index and
//     writes it into `params.frame.x`; the shader only does the cell-UV arithmetic;
//   * alpha EROSION, never a uniform fade: alpha = saturate((signal - erosion) * sharpness), the
//     erosion threshold driven by particle age on the CPU — smoke dissolves with detail instead of
//     ghosting out;
//   * gradient-map coloring: the sprite's grayscale signal indexes the LUT's X, the particle's
//     life fraction indexes Y; the LUT's alpha channel is a HEAT term that multiplies the color
//     above 1.0 (params.glow.x) so young/hot pixels ride into bloom.
//
// Vertex stage is Bevy's default mesh vertex shader; the material is mounted only by the windowed
// clients (vfx::plugin), never the headless server.

#import bevy_pbr::forward_io::VertexOutput

struct VfxParams {
    // x: current flipbook frame (already wrapped on the CPU), y: atlas columns, z: atlas rows,
    // w: unused (frame count lives CPU-side where the wrap happens).
    frame: vec4<f32>,
    // x: erosion threshold 0..1, y: erosion sharpness, z: life fraction 0..1 (gradient LUT row),
    // w: overall alpha multiplier.
    fade: vec4<f32>,
    // x: emissive boost at LUT heat 1.0 (rgb *= 1 + heat * boost), y/z/w: reserved.
    glow: vec4<f32>,
}

@group(3) @binding(0) var<uniform> params: VfxParams;
@group(3) @binding(1) var atlas_texture: texture_2d<f32>;
@group(3) @binding(2) var atlas_sampler: sampler;
@group(3) @binding(3) var lut_texture: texture_2d<f32>;
@group(3) @binding(4) var lut_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let cols = params.frame.y;
    let rows = params.frame.z;
    let frame = params.frame.x;
    let col = frame % cols;
    let row = floor(frame / cols);
    let cell_uv = (in.uv + vec2(col, row)) / vec2(cols, rows);
    let tex = textureSample(atlas_texture, atlas_sampler, cell_uv);
    // The sprite's grayscale signal. The Kenney transparent sprites carry their shape detail in
    // ALPHA over near-white RGB, so r*a reads them correctly — and also reads gray-on-alpha
    // authored sprites, should one join the atlas later.
    let signal = tex.r * tex.a;
    // Alpha erosion (dissolve) — the whole point; never a uniform fade on smoke.
    let alpha = clamp((signal - params.fade.x) * params.fade.y, 0.0, 1.0);
    // Gradient map: X = grayscale signal, Y = life fraction. LUT alpha = heat.
    let g = textureSample(lut_texture, lut_sampler, vec2(signal, params.fade.z));
    let rgb = g.rgb * (1.0 + g.a * params.glow.x);
    return vec4(rgb, alpha * params.fade.w);
}
