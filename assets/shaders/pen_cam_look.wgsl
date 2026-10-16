// The pen cam's glass looks (`src/damage_cam/pen_cam/look.rs`, `LookMaterial`): one material, its
// drawing chosen per instance by `mode.x`.
//
//   0 hull mask   rgb: view-space normal (0..1), a: distance (m) from the eye. Opaque.
//   1 part mask   rgb: the colour, a: distance (m) from the eye. Opaque.
//   2 glass       rim-lit glass drawn only where it is the hull mask's nearest surface, so the hull
//                 shows one layer deep; optional world-space scanlines (`field.w`, period m) and a
//                 clearing around the struck components' screen positions (`hits`).
//   3 dither      a screen-stable 4x4 Bayer screen door keeping `color.a` of the pixels. Opaque.
//   6 dissolve    a field shell, its alpha falling as 1/r^2 from the origin, the band behind its
//                 front (`cone2.z`, m) dissolved by interleaved-gradient noise.
//   7 march       the analytic cone (`cone`: axis, half-angle) integrated along the eye ray behind
//                 the proxy's back face, density 1/r^2 from the origin; alpha or additive.
//   8 soft        a field shell faded where it nears the part mask's surfaces and face-on.
//   9 base        one hueless surface, drawn only where it is its mask's nearest surface (the hull
//                 mask, or the part mask with `mode.z = 1`): half-Lambert shading with `shade.y` the
//                 light's share, opacity `color.a` at a grazing angle falling to `shade.x` of it
//                 face-on (power `shade.z`), and that mask's depth edges in `edge` (rgb, a:
//                 opacity). Where the surface is not its mask's nearest, only its field footprint
//                 draws.
//  10 lines       full screen: depth and normal edges of the hull mask, the depth threshold
//                 growing with distance.
//  11 silhouette  full screen: the hull mask's outer edge.
//  12 state       a component on the state scale, opaque: below its fill level `color` (red, or
//                 charcoal under an `edge`-coloured diagonal hatch when `mode.w = 1`, period
//                 `mode.z` px, `cone2.w` of it hatched), above it `rest` (the untouched grey), a
//                 line of `edge` `span.w` px wide at the level, all half-Lambert shaded with
//                 `shade.y` the light's share. The level rises along `state.xyz` (world up, unit)
//                 from `span.x` to `span.y` (world m) by `state.w` (the share lost) times the fade;
//                 with `span.z = 0` (no known heights) the fill covers the mesh once the fade is on.
//
// Base and state surfaces carry the spall fields' footprint (`foot.x`: 0 none, 1 outline, 2 hatch)
// in `foot_color`: for each of `foot.y` fields (`prints`), a surface point facing the origin,
// inside the cone, behind its front and within the reach of the field's recorded direction (`rays`)
// nearest it is inside the footprint. The outline is the band `foot.z` px wide where the angle
// from the axis crosses the half-angle; the hatch is diagonal stripes `foot.w` px apart over the
// inside, `cone2.z` of each in colour.
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
    // x: gain (march); y: additive (1) or blended (0); z: dissolve band (m), reach (march, m) or
    // the footprint's hatched share (base, state); w: the hatched share (state).
    cone2: vec4<f32>,
    // x: mode; y: hit clearing on (1); z: the part mask (base, 1) or the hatch period (state, px);
    // w: hatch on (1).
    mode: vec4<f32>,
    // xy: a struck component's centre (NDC), z: its radius (NDC height units), w: on (1).
    hits: array<vec4<f32>, 8>,
    // xyz: the fill's up (world, unit); w: the share of the height filled.
    state: vec4<f32>,
    // x, y: the low and high heights along the up (world m); z: heights known (1); w: the fill
    // line's width (px).
    span: vec4<f32>,
    // rgb: the unfilled remainder's colour.
    rest: vec4<f32>,
    // x: footprint style; y: the fields marked; z: outline width (px); w: hatch period (px).
    foot: vec4<f32>,
    // rgb: the footprint's colour; a: its opacity.
    foot_color: vec4<f32>,
    // Per field, three lanes: origin (world) and half-angle (rad); unit axis (world) and reach
    // (m); x: how far its front has run (m), negative before it is thrown, y: its directions.
    prints: array<vec4<f32>, 12>,
    // Per field, 32 lanes: a recorded direction (world, unit) and its reach (m) with slack.
    rays: array<vec4<f32>, 128>,
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
    if mode() == 10 || mode() == 11 {
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

// A mask's edge at a pixel: 1 at a depth edge, 0.5 at a normal edge alone, 0 elsewhere.
// The footprint's mark at world point `p` (normal `n`; its change across a pixel `dpx`, `dpy`) and
// pixel `frag`, 1 or 0.
fn footprint(p: vec3<f32>, n: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>, frag: vec2<f32>) -> f32 {
    let style = i32(look.foot.x + 0.5);
    if style == 0 {
        return 0.0;
    }
    for (var i = 0; i < 4; i = i + 1) {
        if f32(i) >= look.foot.y {
            break;
        }
        let at = look.prints[3 * i];
        let along = look.prints[3 * i + 1];
        let d = p - at.xyz;
        let r = length(d);
        if r < 1e-4 || r > min(along.w, look.prints[3 * i + 2].x) || dot(n, -d) <= 0.0 {
            continue;
        }
        let dir = d / r;
        var best = -2.0;
        var reach = along.w;
        for (var j = 0; j < 32; j = j + 1) {
            if f32(j) >= look.prints[3 * i + 2].y {
                break;
            }
            let ray = look.rays[32 * i + j];
            let k = dot(dir, ray.xyz);
            if k > best {
                best = k;
                reach = ray.w;
            }
        }
        if r > reach {
            continue;
        }
        let c = clamp(dot(dir, along.xyz), -1.0, 1.0);
        let angle = acos(c);
        if style == 1 {
            // The angle's gradient in space, (c d/r - axis) / (r sin θ), across the pixel.
            let g = (c * dir - along.xyz) / max(r * sqrt(max(1.0 - c * c, 1e-8)), 1e-6);
            let per_px = abs(dot(g, dpx)) + abs(dot(g, dpy));
            if abs(angle - at.w) < 0.5 * look.foot.z * per_px {
                return 1.0;
            }
        } else if angle <= at.w && fract((frag.x - frag.y) / look.foot.w) < look.cone2.z {
            return 1.0;
        }
    }
    return 0.0;
}

fn lines(mask: texture_2d<f32>, frag: vec2<f32>) -> f32 {
    let p = vec2<i32>(frag);
    let c = mask_at(mask, p);
    var gd = vec2(0.0);
    var gn = vec3(0.0);
    var gnx = vec3(0.0);
    var cover = 0.0;
    var k = array<f32, 3>(1.0, 2.0, 1.0);
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let s = mask_at(mask, p + vec2(i, j));
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
    return select(select(0.0, 0.5, normal_edge), 1.0, depth_edge);
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
    // The state fill's height, and its change across a pixel, taken in uniform control flow.
    let height = dot(in.world_position.xyz, look.state.xyz);
    let height_px = fwidth(height);
    let dpx = dpdx(in.world_position.xyz);
    let dpy = dpdy(in.world_position.xyz);

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
    if m == 9 {
        let p = vec2<i32>(frag);
        var nearest = 0.0;
        var edge = 0.0;
        if look.mode.z > 0.5 {
            nearest = mask_at(part_mask, p).a;
            edge = lines(part_mask, frag);
        } else {
            nearest = mask_at(hull_mask, p).a;
            edge = lines(hull_mask, frag);
        }
        let mark = footprint(in.world_position.xyz, n, dpx, dpy, frag) * look.foot_color.a;
        if nearest > 0.0 && dist > nearest * 1.002 + 0.003 {
            if mark <= 0.0 {
                discard;
            }
            return out_color(look.foot_color.rgb, mark * fade);
        }
        let toward = select(-n, n, dot(n, v) >= 0.0);
        let l = normalize(vec3(0.4, 1.0, 0.3));
        let lambert = 0.5 * (dot(toward, l) + 1.0);
        var rgb = look.color.rgb * (1.0 - look.shade.y + look.shade.y * lambert);
        var a = look.color.a * mix(look.shade.x, 1.0, pow(1.0 - facing, look.shade.z));
        if edge > 0.75 {
            rgb = look.edge.rgb;
            a = max(a, look.edge.a);
        }
        rgb = mix(rgb, look.foot_color.rgb, mark);
        a = max(a, mark);
        return out_color(rgb, clamp(a, 0.0, 1.0) * fade);
    }
    if m == 3 {
        if bayer(frag) >= look.color.a {
            discard;
        }
        return vec4(look.color.rgb * (0.75 + 0.25 * facing), 1.0);
    }
    if m == 12 {
        let filled = look.state.w * fade;
        var level = -1e9;
        if look.span.z > 0.5 {
            level = mix(look.span.x, look.span.y, filled);
        } else if filled > 0.0 {
            level = 1e9;
        }
        var rgb = look.rest.rgb;
        if height <= level {
            rgb = look.color.rgb;
            if look.mode.w > 0.5 && fract((frag.x + frag.y) / look.mode.z) < look.cone2.w {
                rgb = look.edge.rgb;
            }
        }
        let toward = select(-n, n, dot(n, v) >= 0.0);
        let l = normalize(vec3(0.4, 1.0, 0.3));
        rgb = rgb * (1.0 - look.shade.y + look.shade.y * 0.5 * (dot(toward, l) + 1.0));
        let partial = look.span.z > 0.5 && filled > 0.0 && filled < 1.0;
        if partial && abs(height - level) < 0.5 * look.span.w * height_px {
            rgb = look.edge.rgb;
        }
        let mark = footprint(in.world_position.xyz, n, dpx, dpy, frag) * look.foot_color.a;
        return vec4(mix(rgb, look.foot_color.rgb, mark), 1.0);
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
        if lines(hull_mask, frag) < 0.5 {
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
