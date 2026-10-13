// The pen cam's glass looks (`src/damage_cam/pen_cam/look.rs`, `LookMaterial`): one material, its
// drawing chosen per instance by `mode.x`.
//
//   0 hull mask   rgb: view-space normal (0..1), a: distance (m) from the eye. Opaque.
//   1 part mask   rgb: the colour, a: distance (m) from the eye. Opaque.
//   2 glass       rim-lit glass drawn only where it is the hull mask's nearest surface, so the hull
//                 shows one layer deep; optional world-space scanlines (`field.w`, period m) and a
//                 clearing around the struck components' screen positions (`hits`).
//   3 dither      a screen-stable 4x4 Bayer screen door keeping `color.a` of the pixels. Opaque.
//   4 fill        flat colour, optionally Gooch-toned (`cone2.w`), edged (`edge`), flickering
//                 (`mode.z`) or hatched (`mode.w`).
//   5 hidden      flat colour at `color.a`; its pipeline passes only where something nearer hides it.
//   6 dissolve    a field shell, its alpha falling as 1/r^2 from the origin, the band behind its
//                 front (`cone2.z`, m) dissolved by interleaved-gradient noise.
//   7 march       the analytic cone (`cone`: axis, half-angle) integrated along the eye ray behind
//                 the proxy's back face, density 1/r^2 from the origin; alpha or additive.
//   8 soft        a field shell faded where it nears the part mask's surfaces and face-on.
//  10 lines       full screen: depth and normal edges of the hull mask, the depth threshold
//                 growing with distance.
//  11 silhouette  full screen: the hull mask's outer edge.
//
// Output: additive (`flags.y = 1`) writes `rgb` with alpha 0 for `AlphaMode::Add`; otherwise
// straight `rgb, a`.

#import bevy_pbr::{forward_io::Vertex, mesh_functions, mesh_view_bindings::view}

struct LookParams {
    color: vec4<f32>,
    // rgb: the edge or line colour; a: edge on (1) or off (0).
    edge: vec4<f32>,
    // x: base intensity, y: rim gain, z: rim power, w: fade.
    shade: vec4<f32>,
    // x: front radius (m) from the mesh's origin, negative for none; y: falloff radius r0 (m), 0
    // for none; z: corner time (s); w: scanline period (m), 0 for none.
    field: vec4<f32>,
    // xyz: the cone's axis in the mesh's frame; w: its half-angle (rad).
    cone: vec4<f32>,
    // x: gain (march); y: additive (1) or blended (0); z: dissolve band (m) or reach (march, m);
    // w: Gooch (1) or flat (0).
    cone2: vec4<f32>,
    // x: mode; y: hit clearing on (1); z: flicker on (1); w: hatch on (1).
    mode: vec4<f32>,
    // xy: a struck component's centre (NDC), z: its radius (NDC height units), w: on (1).
    hits: array<vec4<f32>, 8>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> look: LookParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var hull_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var part_mask: texture_2d<f32>;

struct LookVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    // Distance (m, world) from the mesh's origin.
    @location(3) radius: f32,
    @location(4) origin: vec3<f32>,
    @location(5) axis: vec3<f32>,
    // World metres per mesh unit along the axis.
    @location(6) scale: f32,
}

fn mode() -> i32 {
    return i32(look.mode.x + 0.5);
}

@vertex
fn vertex(vertex: Vertex) -> LookVertexOutput {
    var out: LookVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var local = vertex.position;
    if look.field.x >= 0.0 {
        let reach = length((world_from_local * vec4(local, 0.0)).xyz);
        if reach > look.field.x {
            local = local * (look.field.x / reach);
        }
    }
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(local, 1.0));
    out.position = view.clip_from_world * vec4(out.world_position.xyz, 1.0);
    if mode() >= 10 {
        // A 2 x 2 quad in its own frame covers the screen.
        out.position = vec4(vertex.position.xy, 0.5, 1.0);
    }
    out.origin = (world_from_local * vec4(0.0, 0.0, 0.0, 1.0)).xyz;
    out.radius = length(out.world_position.xyz - out.origin);
    let axis = (world_from_local * vec4(look.cone.xyz, 0.0)).xyz;
    out.scale = length(axis);
    out.axis = axis / max(out.scale, 1e-6);
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#else
    out.world_normal = vec3(0.0, 1.0, 0.0);
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#else
    out.color = vec4(1.0);
#endif
    return out;
}

// The 4x4 Bayer threshold at a pixel, in (0, 1).
fn bayer(p: vec2<f32>) -> f32 {
    let x = u32(p.x) % 4u;
    let y = u32(p.y) % 4u;
    var m = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    return (m[y * 4u + x] + 0.5) / 16.0;
}

// Interleaved gradient noise (Jimenez 2014): a cheap screen-space noise with a blue-ish spectrum.
fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2(0.06711056, 0.00583715))));
}

fn mask_at(tex: texture_2d<f32>, p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(tex));
    return textureLoad(tex, clamp(p, vec2(0), size - 1), 0);
}

fn out_color(rgb: vec3<f32>, a: f32) -> vec4<f32> {
    if look.cone2.y > 0.5 {
        return vec4(rgb * a, 0.0);
    }
    return vec4(rgb, a);
}

fn falloff(r: f32) -> f32 {
    if look.field.y <= 0.0 {
        return 1.0;
    }
    let k = look.field.y / max(r, 1e-4);
    return min(1.0, k * k);
}

// The fraction of the struck components' screen clearings this pixel keeps: 1 away from them.
fn clearing(frag: vec2<f32>) -> f32 {
    let size = view.viewport.zw;
    let ndc = vec2(frag.x / size.x * 2.0 - 1.0, 1.0 - frag.y / size.y * 2.0);
    let aspect = size.x / size.y;
    var keep = 1.0;
    for (var i = 0; i < 8; i = i + 1) {
        let hit = look.hits[i];
        if hit.w < 0.5 {
            continue;
        }
        let d = length((ndc - hit.xy) * vec2(aspect, 1.0));
        keep = min(keep, smoothstep(hit.z * 0.8, hit.z * 1.8, d));
    }
    return keep;
}

fn lines(frag: vec2<f32>) -> f32 {
    let p = vec2<i32>(frag);
    let c = mask_at(hull_mask, p);
    var gd = vec2(0.0);
    var gn = vec3(0.0);
    var gnx = vec3(0.0);
    var cover = 0.0;
    var k = array<f32, 3>(1.0, 2.0, 1.0);
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let s = mask_at(hull_mask, p + vec2(i, j));
            let wx = f32(i) * k[j + 1];
            let wy = f32(j) * k[i + 1];
            // An empty pixel stands at a far distance, so the silhouette is a depth edge.
            let d = select(1000.0, s.a, s.a > 0.0);
            gd = gd + vec2(wx, wy) * d;
            gn = gn + wx * s.rgb;
            gnx = gnx + wy * s.rgb;
            cover = cover + select(0.0, 1.0, s.a > 0.0);
        }
    }
    if cover < 0.5 {
        return 0.0;
    }
    let center = select(1000.0, c.a, c.a > 0.0);
    let near = min(center, 1000.0);
    // The depth threshold grows with distance: a crease 1 % of the way back is an edge.
    let depth_edge = length(gd) / max(near, 0.5) > 0.08;
    let normal_edge = length(gn) + length(gnx) > 0.9;
    return select(0.0, 1.0, depth_edge || normal_edge);
}

@fragment
fn fragment(
    in: LookVertexOutput,
    @builtin(front_facing) is_front: bool,
) -> @location(0) vec4<f32> {
    let m = mode();
    let frag = in.position.xy;
    let fade = look.shade.w;
    let to_eye = view.world_position - in.world_position.xyz;
    let dist = length(to_eye);
    let v = to_eye / max(dist, 1e-6);
    let n = normalize(in.world_normal);
    let facing = abs(dot(n, v));
    let rim = look.shade.y * pow(1.0 - facing, look.shade.z);

    if m == 0 {
        let vn = normalize((view.view_from_world * vec4(n, 0.0)).xyz);
        return vec4(vn * 0.5 + 0.5, dist);
    }
    if m == 1 {
        return vec4(look.color.rgb, dist);
    }
    if m == 2 {
        let nearest = mask_at(hull_mask, vec2<i32>(frag)).a;
        if nearest > 0.0 && dist > nearest * 1.002 + 0.003 {
            discard;
        }
        var intensity = look.shade.x + rim;
        if look.field.w > 0.0 {
            let s = abs(fract(in.world_position.y / look.field.w) - 0.5) * 2.0;
            intensity = intensity * (0.55 + 1.2 * smoothstep(0.7, 1.0, s));
        }
        if look.mode.y > 0.5 {
            intensity = intensity * clearing(frag);
        }
        return out_color(look.color.rgb, clamp(intensity * look.color.a, 0.0, 1.0) * fade);
    }
    if m == 3 {
        if bayer(frag) >= look.color.a {
            discard;
        }
        return vec4(look.color.rgb * (0.75 + 0.25 * facing), 1.0);
    }
    if m == 4 || m == 5 {
        var rgb = look.color.rgb;
        if look.cone2.w > 0.5 {
            let l = normalize(vec3(0.4, 1.0, 0.3));
            let t = 0.5 * (dot(select(-n, n, is_front), l) + 1.0);
            let cool = vec3(0.05, 0.08, 0.30) + 0.25 * rgb;
            let warm = vec3(0.30, 0.25, 0.0) + 0.6 * rgb;
            rgb = mix(cool, warm, t);
        }
        if look.edge.a > 0.5 {
            rgb = mix(rgb, look.edge.rgb, clamp(rim, 0.0, 1.0));
        }
        if look.mode.z > 0.5 {
            let step_t = floor(look.field.z * 14.0);
            let r = fract(sin(step_t * 12.9898) * 43758.5453);
            rgb = rgb * select(1.0, 0.35, r > 0.6);
        }
        if look.mode.w > 0.5 {
            let stripe = fract((frag.x + frag.y) / 6.0);
            if stripe < 0.3 {
                rgb = rgb * 0.25;
            }
        }
        return out_color(rgb, look.color.a * fade);
    }
    if m == 6 {
        var a = look.color.a * in.color.a * falloff(in.radius);
        if look.field.x >= 0.0 && look.cone2.z > 0.0 {
            let behind = (look.field.x - in.radius) / look.cone2.z;
            if ign(frag) > behind {
                discard;
            }
        }
        return out_color(look.color.rgb, clamp(a, 0.0, 1.0) * fade);
    }
    if m == 7 {
        // March the eye ray from in front of the cone to the proxy's back face.
        let reach = select(1e6, look.field.x, look.field.x >= 0.0);
        let length_m = min(reach, look.cone2.z * in.scale);
        let ray = -v;
        let t1 = dist;
        let t0 = max(0.0, dist - 2.0 * length_m);
        let steps = 32;
        let dt = (t1 - t0) / f32(steps);
        let cos_half = cos(look.cone.w);
        var sum = 0.0;
        for (var i = 0; i < steps; i = i + 1) {
            let p = view.world_position + ray * (t0 + (f32(i) + 0.5) * dt);
            let d = p - in.origin;
            let r = length(d);
            if r > 1e-4 && r <= length_m && dot(d / r, in.axis) >= cos_half {
                sum = sum + falloff(r) * dt;
            }
        }
        let a = 1.0 - exp(-look.cone2.x * sum);
        return out_color(look.color.rgb, a * look.color.a * fade);
    }
    if m == 8 {
        var a = look.color.a * in.color.a * falloff(in.radius) * (0.25 + 0.75 * pow(1.0 - facing, 1.5));
        let scene = mask_at(part_mask, vec2<i32>(frag)).a;
        if scene > 0.0 {
            a = a * clamp((scene - dist) / 0.25, 0.0, 1.0);
        }
        return out_color(look.color.rgb, clamp(a, 0.0, 1.0) * fade);
    }
    if m == 10 {
        if lines(frag) < 0.5 {
            discard;
        }
        return vec4(look.color.rgb, look.color.a);
    }
    if m == 11 {
        let p = vec2<i32>(frag);
        let inside = mask_at(hull_mask, p).a > 0.0;
        if inside {
            discard;
        }
        var near = false;
        for (var j = -2; j <= 2; j = j + 1) {
            for (var i = -2; i <= 2; i = i + 1) {
                if mask_at(hull_mask, p + vec2(i, j)).a > 0.0 {
                    near = true;
                }
            }
        }
        if !near {
            discard;
        }
        return vec4(look.color.rgb, look.color.a);
    }
    discard;
    return vec4(0.0);
}
