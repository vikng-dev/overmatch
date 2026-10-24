// The pen cam's corner look (`src/damage_cam/pen_cam/look.rs`, `LookMaterial`): one material, its
// drawing chosen per instance by `mode.x`.
//
//   0 mask        rgb: view-space normal (0..1), a: the eye depth (m, `eye_depth`). Opaque. The
//                 hull and the part masks are both drawn in it.
//   2 base        one hueless surface, drawn only where it is its mask's nearest surface: a body
//                 surface (`mode.z = 0`) the hull mask's, and nowhere the part mask holds a
//                 component; a component (`mode.z = 1`) the part mask's. Half-Lambert shading with
//                 `shade.y` the light's share, opacity `color.a` at a grazing angle falling to
//                 `shade.x` of it face-on (power `shade.z`), and that mask's depth edges in `edge`
//                 (rgb, a: opacity), under its field footprint. Where the surface is not its mask's
//                 nearest, only the footprint's lines draw.
//   3 state       a component on the state scale, opaque: below its fill level `color` (red, or
//                 charcoal under an `edge`-coloured diagonal hatch when `mode.w = 1`, period
//                 `hatch.x` px, `hatch.y` of it hatched), above it `rest` (the untouched grey), a
//                 line of `edge` `span.w` px wide at the level, all half-Lambert shaded with
//                 `shade.y` the light's share. The level rises along `state.xyz` (world up, unit)
//                 from `span.x` to `span.y` (world m) by `state.w` (the share lost) times the fade;
//                 with `span.z = 0` (no known heights) the fill covers the mesh once the fade is on.
//                 Its field footprint tints it over the fill.
//   4 volume      one spall field's volume (`volume`), drawn on the back faces of its reach
//                 surface, where the eye ray leaves it, whatever stands in front: the ray is
//                 marched back toward the eye in steps of `volume.z` (m), at most `VOLUME_STEPS`,
//                 while it stays inside the field (`sample`: inside the cone, behind the front,
//                 within the disc's reach, on the cut's kept side), where the ray enters it refined
//                 by `VOLUME_REFINE` halvings, the eye the end if it lies inside. With
//                 `volume.w = 1` the pixel's nearest component in the part mask bounds it: the
//                 stretch behind the component is passed over, and the march takes the rest from
//                 the component toward the eye, or nothing where the ray's stretch inside lies
//                 wholly behind it. The pixel takes the length-weighted mean of `ramp` at every
//                 sample's intensity, at opacity `volume.x · (1 − exp(−L / volume.y))` for `L` the
//                 length marched inside, times the fade. Field 0's lanes alone.
//
// In every mode, `mode.y = 1` discards what lies on `plane`'s normal side (the slice's cut).
//
// Draw order (`look.rs`, module doc): the body draws behind the components and the fields, which
// are ordered by depth among themselves. A body surface is discarded wherever the part mask holds
// a component, and a volume is drawn after the body and over it; a component is opaque and
// depth-tested, and a volume's march ends at the nearest component. Every depth read against a
// mask is the eye depth (`eye_depth`): the distance from the eye in a perspective view, the
// distance along the camera's forward from its plane in an orthographic one, the masks' own
// cameras sharing the viewer's pose and lens.
//
// The potency colour (`ramp`; design doc §5, "Potency colour") is one function of intensity
// alone: a monotone cubic in `u = log2(I / T_mid)` through three anchors (`ramp[0..3]`: rgb, and
// `u` in w), level at the ends and at the middle anchor of slope `ramp[3].rgb`, `ramp[3].w` the
// mid threshold `T_mid`; `potency::Ramp` is the same curve. A spall field's intensity at a point
// is the emission at its direction on the disc over the squared distance from its origin.
//
// Base and state surfaces carry the spall fields' footprint (`footprint`): for each of the fields
// (`fields.x`, none at 0; `prints`), a surface point facing the origin, inside the cone, behind its
// front and no farther than its direction's reach on the field's disc (`disc`, with the slack
// `foot.zw`) takes the field's intensity there. The fields' summed intensity fills the point in
// its potency colour at opacity `fill.x`; lines `foot.x` px wide at opacity `foot.y` mark where
// the angle from a cone's axis crosses its half-angle, in the potency colour there. No lane the
// colour is read from carries a display gain.
//
// Output: straight `rgb, a`.

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
    // x: mode; y: cut by `plane` (1); z: a component, tested against the part mask (base, 1), or
    // the body (base, 0); w: hatch on (state, 1).
    mode: vec4<f32>,
    // xyz: the fill's up (world, unit); w: the share of the height filled.
    state: vec4<f32>,
    // x, y: the low and high heights along the up (world m); z: heights known (1); w: the fill
    // line's width (px).
    span: vec4<f32>,
    // rgb: the unfilled remainder's colour.
    rest: vec4<f32>,
    // The cut (`mode.y = 1`): xyz the plane's unit normal toward the discarded side (world), w
    // normal · a point on it.
    plane: vec4<f32>,
    // x: the lines' width (px), y: their opacity; z, w: a surface counts as met up to `z` times its
    // direction's reach plus `w` (m).
    foot: vec4<f32>,
    // x: the footprint fill's opacity.
    fill: vec4<f32>,
    // The potency colour's anchors (rgb, and `u` in w) and its middle slope (rgb, and the mid
    // threshold in w).
    ramp: array<vec4<f32>, 4>,
    // The volume: x its opacity at most, y the length over which it climbs (m), z the march's step
    // (m), w bounded by the part mask's nearest component (1).
    volume: vec4<f32>,
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
// `look::VOLUME_STEPS`, the step being a field's longest chord over it; the halvings that refine
// where the eye ray enters the volume, to an eighth of a step.
const VOLUME_STEPS: i32 = 16;
const VOLUME_REFINE: i32 = 3;

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

fn orthographic() -> bool {
    return view.clip_from_view[3][3] == 1.0;
}

// The eye depth of world point `p` (m): its distance from the eye in a perspective view, its
// distance along the camera's forward from the camera's plane in an orthographic one. The masks
// store it; every test against them reads it.
fn eye_depth(p: vec3<f32>) -> f32 {
    if orthographic() {
        return dot(p - view.world_position, -normalize(view.world_from_view[2].xyz));
    }
    return length(p - view.world_position);
}

// The unit direction of the eye ray through world point `p`, away from the eye: an orthographic
// view's rays are parallel, along its forward. A point `s` nearer the eye on that ray than `p` is
// `p - eye_ray(p) * s`, at eye depth `eye_depth(p) - s`.
fn eye_ray(p: vec3<f32>) -> vec3<f32> {
    if orthographic() {
        return -normalize(view.world_from_view[2].xyz);
    }
    return normalize(p - view.world_position);
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

// The potency colour (linear rgb) at intensity `i` (HP points per m²): `potency::Ramp::colour`.
fn ramp(i: f32) -> vec3<f32> {
    let u = log2(max(i, 1e-30) / look.ramp[3].w);
    if u >= look.ramp[2].w {
        return look.ramp[2].rgb;
    }
    if u <= look.ramp[0].w {
        return look.ramp[0].rgb;
    }
    let k = select(0, 1, u >= look.ramp[1].w);
    let a = look.ramp[k];
    let b = look.ramp[k + 1];
    let span = b.w - a.w;
    let t = (u - a.w) / span;
    let t2 = t * t;
    let t3 = t2 * t;
    // The cubic Hermite between the segment's anchors; only the middle anchor has a slope.
    let slope_a = select(vec3(0.0), look.ramp[3].rgb, k == 1);
    let slope_b = select(vec3(0.0), look.ramp[3].rgb, k == 0);
    return a.rgb * (2.0 * t3 - 3.0 * t2 + 1.0) + slope_a * span * (t3 - 2.0 * t2 + t)
        + b.rgb * (3.0 * t2 - 2.0 * t3) + slope_b * span * (t3 - t2);
}

// Unit direction `dir` from field `f`'s origin on its disc: the disc point (xy) and the angle
// from the axis (z, rad). Past the half-angle the point is the rim's.
fn polar(f: i32, dir: vec3<f32>) -> vec3<f32> {
    let half_angle = look.prints[4 * f].w;
    let angle = acos(clamp(dot(dir, look.prints[4 * f + 1].xyz), -1.0, 1.0));
    let across = vec2(dot(dir, look.prints[4 * f + 2].xyz), dot(dir, look.prints[4 * f + 3].xyz));
    let s = length(across);
    let q = select(vec2(0.0), across / max(s, 1e-6) * min(angle / half_angle, 1.0), s > 1e-6);
    return vec3(q, angle);
}

// What the footprint draws at a surface point: the fields' summed intensity there (HP points per
// m², 0 outside every field), and a line there (1, in `line_rgb`) or none (0).
struct Footprint {
    intensity: f32,
    line: f32,
    line_rgb: vec3<f32>,
}

// The footprint at world point `p` (normal `n`; its change across a pixel `dpx`, `dpy`).
fn footprint(p: vec3<f32>, n: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>) -> Footprint {
    var intensity = 0.0;
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
        let on_disc = polar(f, dir);
        let c = clamp(dot(dir, along.xyz), -1.0, 1.0);
        // The angle's gradient in space, (c d/r - axis) / (r sin θ), across the pixel.
        let g = (c * dir - along.xyz) / max(r * sqrt(max(1.0 - c * c, 1e-8)), 1e-6);
        let per_px = abs(dot(g, dpx)) + abs(dot(g, dpy));
        let on_edge = abs(on_disc.z - at.w) < 0.5 * look.foot.x * per_px;
        if on_disc.z > at.w && !on_edge {
            continue;
        }
        let here = disc_at(f, on_disc.xy);
        if r > here.x * look.foot.z + look.foot.w {
            continue;
        }
        edge = edge || on_edge;
        if on_disc.z <= at.w {
            intensity = intensity + here.y / (r * r);
        }
    }
    var out = Footprint(intensity, 0.0, vec3(0.0));
    if edge {
        out.line = 1.0;
        out.line_rgb = ramp(intensity);
    }
    return out;
}

// A point on an eye ray in field 0's volume: whether it lies inside (in the cone, behind the
// front, within its direction's reach on the disc, on the cut's kept side) and its intensity
// there (HP points per m²).
struct Sample {
    inside: bool,
    intensity: f32,
}

fn sample(p: vec3<f32>) -> Sample {
    let at = look.prints[0];
    let front = look.prints[1].w;
    let d = p - at.xyz;
    let r = length(d);
    if r < 1e-5 {
        return Sample(front > 0.0, 1e30);
    }
    let on_disc = polar(0, d / r);
    let here = disc_at(0, on_disc.xy);
    let cut = look.mode.y > 0.5 && dot(look.plane.xyz, p) > look.plane.w;
    let inside = on_disc.z <= at.w && r <= min(here.x, front) && !cut;
    return Sample(inside, here.y / (r * r));
}

// Field 0's volume seen through `exit`, a point where the eye ray leaves it, at mask pixel `p`:
// the ray marched back toward the eye from the exit, or from the nearest component where one
// stands in front of the exit, its colour the length-weighted mean of the potency colour along it
// (the trapezoid rule between samples), its opacity climbing with the length inside.
fn volume(exit: vec3<f32>, p: vec2<i32>, fade: f32) -> vec4<f32> {
    // Depths along the ray are eye depths: `t` lies `t_exit - t` nearer the eye than the exit.
    let dir = eye_ray(exit);
    let t_exit = eye_depth(exit);
    var far = t_exit;
    if look.volume.w > 0.5 {
        let bound = mask_at(part_mask, p).a;
        if bound > 0.0 && t_exit > bound * 1.002 + 0.003 {
            far = bound;
        }
    }
    let step = look.volume.z;
    var last_t = far;
    let start = sample(exit - dir * (t_exit - far));
    // A component in front of the exit with the field's edge between them: the field this exit
    // closes lies wholly behind it.
    if far < t_exit && !start.inside {
        discard;
    }
    var last_rgb = ramp(start.intensity);
    var colour = vec3(0.0);
    var inside = 0.0;
    for (var k = 1; k <= VOLUME_STEPS; k = k + 1) {
        let t = max(t_exit - f32(k) * step, 0.0);
        let here = sample(exit - dir * (t_exit - t));
        if t >= far {
            // Behind the component: passed over, the ray only held to the field this exit closes.
            if !here.inside {
                discard;
            }
            continue;
        }
        if here.inside {
            let rgb = ramp(here.intensity);
            colour = colour + 0.5 * (rgb + last_rgb) * (last_t - t);
            inside = inside + (last_t - t);
            last_rgb = rgb;
            last_t = t;
            if t <= 0.0 {
                break;
            }
            continue;
        }
        // The ray came in between `t` and `last_t`: halve that span toward the crossing.
        var lo = t;
        var hi = last_t;
        var hi_intensity = -1.0;
        for (var j = 0; j < VOLUME_REFINE; j = j + 1) {
            let mid = 0.5 * (lo + hi);
            let probe = sample(exit - dir * (t_exit - mid));
            if probe.inside {
                hi = mid;
                hi_intensity = probe.intensity;
            } else {
                lo = mid;
            }
        }
        let entry = 0.5 * (lo + hi);
        let near_rgb = select(last_rgb, ramp(hi_intensity), hi_intensity >= 0.0);
        colour = colour + 0.5 * (near_rgb + last_rgb) * (last_t - entry);
        inside = inside + (last_t - entry);
        break;
    }
    if inside <= 0.0 {
        discard;
    }
    let a = look.volume.x * (1.0 - exp(-inside / look.volume.y)) * fade;
    return vec4(colour / inside, clamp(a, 0.0, 1.0));
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
    let depth = eye_depth(in.world_position.xyz);

    if look.mode.y > 0.5 && dot(look.plane.xyz, in.world_position.xyz) > look.plane.w {
        discard;
    }
    if m == 0 {
        let vn = normalize((view.view_from_world * vec4(n, 0.0)).xyz);
        // Never 0, the empty pixel's depth.
        return vec4(vn * 0.5 + 0.5, max(depth, 1e-4));
    }
    if m == 2 {
        let p = vec2<i32>(pix);
        var nearest = 0.0;
        var edge = 0.0;
        if look.mode.z > 0.5 {
            nearest = mask_at(part_mask, p).a;
            edge = lines(part_mask, pix);
        } else {
            // The body is behind every component.
            if mask_at(part_mask, p).a > 0.0 {
                discard;
            }
            nearest = mask_at(hull_mask, p).a;
            edge = lines(hull_mask, pix);
        }
        let mark = footprint(in.world_position.xyz, n, dpx, dpy);
        // Beneath the nearest layer only the footprint's lines draw.
        if nearest > 0.0 && depth > nearest * 1.002 + 0.003 {
            if mark.line == 0.0 {
                discard;
            }
            return vec4(mark.line_rgb, look.foot.y * fade);
        }
        var rgb = look.color.rgb * shading;
        var a = look.color.a * mix(look.shade.x, 1.0, pow(1.0 - facing, look.shade.z));
        if edge > 0.75 {
            rgb = look.edge.rgb;
            a = max(a, look.edge.a);
        }
        if mark.intensity > 0.0 {
            rgb = mix(rgb, ramp(mark.intensity), look.fill.x);
            a = max(a, look.fill.x);
        }
        if mark.line > 0.0 {
            rgb = mix(rgb, mark.line_rgb, look.foot.y);
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
        if mark.intensity > 0.0 {
            rgb = mix(rgb, ramp(mark.intensity), look.fill.x);
        }
        if mark.line > 0.0 {
            rgb = mix(rgb, mark.line_rgb, look.foot.y);
        }
        return vec4(rgb, 1.0);
    }
    if m == 4 {
        return volume(in.world_position.xyz, vec2<i32>(pix), fade);
    }
    discard;
    return vec4(0.0);
}
