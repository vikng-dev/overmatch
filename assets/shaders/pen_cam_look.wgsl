// The pen cam's corner look (`src/damage_cam/pen_cam/look.rs`, `LookMaterial`): one material, its
// drawing chosen per instance by `mode.x`.
//
//   0 mask        rgb: view-space normal (0..1), a: distance (m) from the eye. Opaque. The hull
//                 and the part masks are both drawn in it.
//   2 base        one hueless surface, drawn only where it is its mask's nearest surface (the hull
//                 mask, or the part mask with `mode.z = 1`): half-Lambert shading with `shade.y` the
//                 light's share, opacity `color.a` at a grazing angle falling to `shade.x` of it
//                 face-on (power `shade.z`), and that mask's depth edges in `edge` (rgb, a:
//                 opacity), under its field footprint. Where the surface is not its mask's
//                 nearest, only the footprint's lines draw.
//   3 state       a component on the state scale, opaque: below its fill level `color` (red, or
//                 charcoal under an `edge`-coloured diagonal hatch when `mode.w = 1`, period
//                 `hatch.x` px, `hatch.y` of it hatched), above it `rest` (the untouched grey), a
//                 line of `edge` `span.w` px wide at the level, all half-Lambert shaded with
//                 `shade.y` the light's share. The level rises along `state.xyz` (world up, unit)
//                 from `span.x` to `span.y` (world m) by `state.w` (the share lost) times the fade;
//                 with `span.z = 0` (no known heights) the fill covers the mesh once the fade is on.
//                 Its field footprint tints it over the fill.
//   4 shell       one potency band's shell of a spall field (design doc §5, "Potency bands"):
//                 `color.rgb` at gain `color.a`, `shade.x` of it face-on rising to the whole at a
//                 grazing angle by power `shade.z`. Additive; the material says whether it is
//                 occluded.
//
// Base and state surfaces carry the spall fields' footprint (`footprint`; design doc §5, "Potency
// bands"): for each of the fields (`fields.x`, none at 0; `prints`), a surface point facing the
// origin, inside the cone, behind its front and no farther than its direction's reach on the
// field's disc (`disc`, with the slack `foot.zw`) takes the field's intensity there, emission over
// the squared distance. The summed intensity's potency band (`bands`: the mid and bright
// thresholds) fills the point in that band's tint (`tints`: dim, mid, bright; a: the fill's
// opacity), and lines `foot.x` px wide at opacity `foot.y` mark where the band changes and where
// the angle from a cone's axis crosses its half-angle. No lane the band is read from carries a
// display gain.
//
// Output: straight `rgb, a`; additive (shell) `rgb` with alpha 0.

#import bevy_pbr::{forward_io::Vertex, mesh_functions, mesh_view_bindings::view}

struct LookParams {
    color: vec4<f32>,
    // rgb: the edge or line colour; a: its opacity.
    edge: vec4<f32>,
    // x: face-on opacity share, y: the light's share of the shading, z: fresnel power, w: fade.
    shade: vec4<f32>,
    // x: front radius (m) from the mesh's origin, negative for none.
    field: vec4<f32>,
    // x: the state hatch's period (px); y: its hatched share.
    hatch: vec4<f32>,
    // x: mode; z: the part mask (base, 1); w: hatch on (state, 1).
    mode: vec4<f32>,
    // xyz: the fill's up (world, unit); w: the share of the height filled.
    state: vec4<f32>,
    // x, y: the low and high heights along the up (world m); z: heights known (1); w: the fill
    // line's width (px).
    span: vec4<f32>,
    // rgb: the unfilled remainder's colour.
    rest: vec4<f32>,
    // x: the lines' width (px), y: their opacity; z, w: a surface counts as met up to `z` times its
    // direction's reach plus `w` (m).
    foot: vec4<f32>,
    // x: the mid band's threshold, y: the bright band's (HP points per m²).
    bands: vec4<f32>,
    // The dim, mid and bright bands' tints: rgb, a: the fill's opacity.
    tints: array<vec4<f32>, 3>,
    // x: the fields marked.
    fields: vec4<f32>,
    // Four fields of four lanes: origin (world) and half-angle (rad); unit axis (world) and how far
    // its front has run (m), negative before it is thrown; the disc's `u` and `v` (world, unit).
    prints: array<vec4<f32>, 16>,
    // Four fields of `DISC_LANES` lanes, two disc nodes a lane: each node's reach (m) and emission
    // (HP points per sr, world), row by row from `q.y = −1` (`field::Disc`).
    disc: array<vec4<f32>, 1152>,
}

// `field::DISC_SIDE` and the lanes one field's disc takes.
const DISC_SIDE: i32 = 24;
const DISC_LANES: i32 = 288;

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> look: LookParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var hull_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var part_mask: texture_2d<f32>;

struct LookVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
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
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#else
    out.world_normal = vec3(0.0, 1.0, 0.0);
#endif
    return out;
}

fn mask_at(tex: texture_2d<f32>, p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(tex));
    return textureLoad(tex, clamp(p, vec2(0), size - 1), 0);
}

// Field `f`'s disc node `(i, j)`: its reach (m) and emission (HP points per sr).
fn node(f: i32, i: i32, j: i32) -> vec2<f32> {
    let k = j * DISC_SIDE + i;
    let lane = look.disc[f * DISC_LANES + k / 2];
    return select(lane.xy, lane.zw, (k & 1) == 1);
}

// Field `f`'s disc at point `q`, bilinear between its four nodes, `q` clamped to the grid:
// `field::Disc::sample`, read the same way.
fn disc_at(f: i32, q: vec2<f32>) -> vec2<f32> {
    let top = f32(DISC_SIDE - 1);
    let g = clamp((q + 1.0) * 0.5 * top, vec2(0.0), vec2(top));
    let low = min(floor(g), vec2(top - 1.0));
    let t = g - low;
    let i = i32(low.x);
    let j = i32(low.y);
    let below = mix(node(f, i, j), node(f, i + 1, j), t.x);
    let above = mix(node(f, i, j + 1), node(f, i + 1, j + 1), t.x);
    return mix(below, above, t.y);
}

// What the footprint draws at a surface point: its potency band (0 none, 1 dim, 2 mid, 3 bright)
// and the band whose tint a line there takes, 0 for no line.
struct Footprint {
    band: i32,
    line: i32,
}

// The footprint at world point `p` (normal `n`; its change across a pixel `dpx`, `dpy`).
fn footprint(p: vec3<f32>, n: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>) -> Footprint {
    var intensity = 0.0;
    // The summed intensity's change across a pixel along each field's radius, `2 I / r` a metre.
    var change = 0.0;
    var edge = false;
    for (var f = 0; f < 4; f = f + 1) {
        if f32(f) >= look.fields.x {
            break;
        }
        let at = look.prints[4 * f];
        let along = look.prints[4 * f + 1];
        let d = p - at.xyz;
        let r = length(d);
        if r < 1e-4 || r > along.w || dot(n, -d) <= 0.0 {
            continue;
        }
        let dir = d / r;
        let c = clamp(dot(dir, along.xyz), -1.0, 1.0);
        let angle = acos(c);
        // The angle's gradient in space, (c d/r - axis) / (r sin θ), across the pixel.
        let g = (c * dir - along.xyz) / max(r * sqrt(max(1.0 - c * c, 1e-8)), 1e-6);
        let per_px = abs(dot(g, dpx)) + abs(dot(g, dpy));
        let on_edge = abs(angle - at.w) < 0.5 * look.foot.x * per_px;
        if angle > at.w && !on_edge {
            continue;
        }
        let across = vec2(dot(dir, look.prints[4 * f + 2].xyz), dot(dir, look.prints[4 * f + 3].xyz));
        let s = length(across);
        let q = select(vec2(0.0), across / max(s, 1e-6) * min(angle / at.w, 1.0), s > 1e-6);
        let here = disc_at(f, q);
        if r > here.x * look.foot.z + look.foot.w {
            continue;
        }
        edge = edge || on_edge;
        if angle <= at.w {
            let i = here.y / (r * r);
            intensity = intensity + i;
            change = change + 2.0 * i / r * (abs(dot(dir, dpx)) + abs(dot(dir, dpy)));
        }
    }
    var out = Footprint(0, 0);
    if intensity > 0.0 {
        out.band = select(select(1, 2, intensity >= look.bands.x), 3, intensity >= look.bands.y);
        // A threshold's line: where the intensity is within half the line's width of it.
        let half = 0.5 * look.foot.x * change;
        if abs(intensity - look.bands.y) < half {
            out.line = 3;
        } else if abs(intensity - look.bands.x) < half {
            out.line = 2;
        }
    }
    if edge && out.line == 0 {
        out.line = max(out.band, 1);
    }
    return out;
}

// A mask's edge at a pixel: 1 at a depth edge, 0.5 at a normal edge alone, 0 elsewhere.
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
fn fragment(in: LookVertexOutput) -> @location(0) vec4<f32> {
    let m = mode();
    let frag = in.position.xy;
    let fade = look.shade.w;
    let to_eye = view.world_position - in.world_position.xyz;
    let dist = length(to_eye);
    let v = to_eye / max(dist, 1e-6);
    let n = normalize(in.world_normal);
    let facing = abs(dot(n, v));
    // The state fill's height, its change across a pixel, and the position's, taken in uniform
    // control flow.
    let height = dot(in.world_position.xyz, look.state.xyz);
    let height_px = fwidth(height);
    let dpx = dpdx(in.world_position.xyz);
    let dpy = dpdy(in.world_position.xyz);
    let toward = select(-n, n, dot(n, v) >= 0.0);
    let lambert = 0.5 * (dot(toward, normalize(vec3(0.4, 1.0, 0.3))) + 1.0);
    let shading = 1.0 - look.shade.y + look.shade.y * lambert;
    // Where the masks hold this fragment: its position within its own viewport.
    let pix = frag - view.viewport.xy;

    if m == 0 {
        let vn = normalize((view.view_from_world * vec4(n, 0.0)).xyz);
        return vec4(vn * 0.5 + 0.5, dist);
    }
    if m == 2 {
        let p = vec2<i32>(pix);
        var nearest = 0.0;
        var edge = 0.0;
        if look.mode.z > 0.5 {
            nearest = mask_at(part_mask, p).a;
            edge = lines(part_mask, pix);
        } else {
            nearest = mask_at(hull_mask, p).a;
            edge = lines(hull_mask, pix);
        }
        let mark = footprint(in.world_position.xyz, n, dpx, dpy);
        // Beneath the nearest layer only the footprint's lines draw.
        if nearest > 0.0 && dist > nearest * 1.002 + 0.003 {
            if mark.line == 0 {
                discard;
            }
            return vec4(look.tints[mark.line - 1].rgb, look.foot.y * fade);
        }
        var rgb = look.color.rgb * shading;
        var a = look.color.a * mix(look.shade.x, 1.0, pow(1.0 - facing, look.shade.z));
        if edge > 0.75 {
            rgb = look.edge.rgb;
            a = max(a, look.edge.a);
        }
        if mark.band > 0 {
            let tint = look.tints[mark.band - 1];
            rgb = mix(rgb, tint.rgb, tint.a);
            a = max(a, tint.a);
        }
        if mark.line > 0 {
            rgb = mix(rgb, look.tints[mark.line - 1].rgb, look.foot.y);
            a = max(a, look.foot.y);
        }
        return vec4(rgb, clamp(a, 0.0, 1.0) * fade);
    }
    if m == 3 {
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
            if look.mode.w > 0.5 && fract((frag.x + frag.y) / look.hatch.x) < look.hatch.y {
                rgb = look.edge.rgb;
            }
        }
        rgb = rgb * shading;
        let partial = look.span.z > 0.5 && filled > 0.0 && filled < 1.0;
        if partial && abs(height - level) < 0.5 * look.span.w * height_px {
            rgb = look.edge.rgb;
        }
        let mark = footprint(in.world_position.xyz, n, dpx, dpy);
        if mark.band > 0 {
            let tint = look.tints[mark.band - 1];
            rgb = mix(rgb, tint.rgb, tint.a);
        }
        if mark.line > 0 {
            rgb = mix(rgb, look.tints[mark.line - 1].rgb, look.foot.y);
        }
        return vec4(rgb, 1.0);
    }
    if m == 4 {
        let rim = mix(look.shade.x, 1.0, pow(1.0 - facing, look.shade.z));
        return vec4(look.color.rgb * look.color.a * rim * fade, 0.0);
    }
    discard;
    return vec4(0.0);
}
