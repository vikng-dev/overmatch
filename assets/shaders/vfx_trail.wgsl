// Shell smoke-trail ribbon fragment (src/vfx/trail.rs — `VfxTrailMaterial`).
//
// The ribbon mesh (rebuilt on the CPU each frame, camera-facing) carries per-vertex data:
//   uv.x    — 0..1 across the strip (0/1 at the edges, 0.5 on the spine);
//   uv.y    — arc length along the trail in meters, anchored at CAPTURE time so the noise pattern
//             sticks to the air the shell flew through instead of swimming when tail points expire;
//   color.r — the point's age fraction 0..1 (0 = at the shell) — the erosion driver;
//   color.g — a per-point random seed, offsetting the noise row so two trails never match.
//
// Fragment recipe (Klemen Lozar's smoke-breakup shape): a soft across-strip profile modulated by
// two scrolling octaves of tileable noise gives the density signal; the erosion threshold rises
// with point age so the tail dissolves with detail; a gradient LUT (X = signal, Y = age) colors it.
// ONE material instance is shared by every live trail — time comes from the view globals, ages from
// the vertices, so nothing here is per-instance.

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::globals

struct TrailParams {
    // x: noise scale along the trail (1/m), y: noise scroll speed (tiles/s), z: erosion sharpness,
    // w: overall alpha multiplier.
    shape: vec4<f32>,
    // x: emissive boost at LUT heat 1.0, y: erosion floor at age 0 (fresh trail is already this
    // eroded, so it never reads as a solid tube), z/w: reserved.
    glow: vec4<f32>,
}

@group(3) @binding(0) var<uniform> params: TrailParams;
@group(3) @binding(1) var noise_texture: texture_2d<f32>;
@group(3) @binding(2) var noise_sampler: sampler;
@group(3) @binding(3) var lut_texture: texture_2d<f32>;
@group(3) @binding(4) var lut_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let age = in.color.r;
    let seed = in.color.g;
    // Soft profile across the strip: 1 on the spine, 0 at the edges.
    let across = 1.0 - abs(in.uv.x * 2.0 - 1.0);
    // Two octaves of tileable noise, scrolled in opposite directions along the trail.
    let n1 = textureSample(
        noise_texture,
        noise_sampler,
        vec2(in.uv.y * params.shape.x + globals.time * params.shape.y, in.uv.x * 0.31 + seed),
    ).r;
    let n2 = textureSample(
        noise_texture,
        noise_sampler,
        vec2(
            in.uv.y * params.shape.x * 2.63 - globals.time * params.shape.y * 1.7 + 0.37,
            in.uv.x * 0.57 + seed * 1.83,
        ),
    ).r;
    let noise = 0.62 * n1 + 0.38 * n2;
    let signal = across * (0.45 + 0.55 * noise);
    // Age-driven erosion: fresh trail keeps a noise-broken floor, old trail erodes away entirely.
    let erosion = params.glow.y + (1.0 - params.glow.y) * age;
    let alpha = clamp((signal - erosion) * params.shape.z, 0.0, 1.0);
    let g = textureSample(lut_texture, lut_sampler, vec2(signal, age));
    let rgb = g.rgb * (1.0 + g.a * params.glow.x);
    return vec4(rgb, alpha * params.shape.w);
}
