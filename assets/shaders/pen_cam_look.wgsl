// The pen cam's corner look (`src/damage_cam/pen_cam/look.rs`, `LookMaterial`): one material, its
// drawing chosen per instance by `mode.x`.
//
//   0 mask        rgb: view-space normal (0..1), a: distance (m) from the eye. Opaque. The hull
//                 and the part masks are both drawn in it.
//   2 base        one hueless surface, drawn only where it is its mask's nearest surface (the hull
//                 mask, or the part mask with `mode.z = 1`): half-Lambert shading with `shade.y` the
//                 light's share, opacity `color.a` at a grazing angle falling to `shade.x` of it
//                 face-on (power `shade.z`), and that mask's depth edges in `edge` (rgb, a:
//                 opacity). Where the surface is not its mask's nearest, only its field footprint
//                 draws.
//   3 state       a component on the state scale, opaque: below its fill level `color` (red, or
//                 charcoal under an `edge`-coloured diagonal hatch when `mode.w = 1`, period
//                 `hatch.x` px, `hatch.y` of it hatched), above it `rest` (the untouched grey), a
//                 line of `edge` `span.w` px wide at the level, all half-Lambert shaded with
//                 `shade.y` the light's share. The level rises along `state.xyz` (world up, unit)
//                 from `span.x` to `span.y` (world m) by `state.w` (the share lost) times the fade;
//                 with `span.z = 0` (no known heights) the fill covers the mesh once the fade is on.
//   4 volume      a field as light: the closed reach mesh's surfaces (wound outward), entries
//                 subtracting and exits adding the column `∫ p ds / (r² + ε²)` from the eye (r from
//                 the origin, ε = `volume.w`), so the sum over a pixel is the density integrated
//                 through the volume; `color.a` is the gain. The profile `p = profile.w + g·x / r`
//                 (`g` = `profile.xyz` in the mesh's frame, x from the origin, r = |x|) depends on
//                 direction only, and ε softens the inverse square alone. It is one function of
//                 position, so the entries and exits cancel outside the volume; both terms integrate
//                 in closed form. Additive, unoccluded.
//
// Base and state surfaces carry the spall fields' footprint (`foot.x`: 0 none, 1 outline) in
// `foot_color`: for each of the fields (`fields.x`, `prints`), a surface point facing the origin,
// inside the cone, behind its front and within the reach of the field's recorded direction (`rays`)
// nearest it is inside the footprint. The outline is the band `foot.y` px wide where the angle from
// the axis crosses the half-angle, at the field's level (its third lane's `z`) of `foot_color.a`.
//
// Output: straight `rgb, a`; additive (volume) `rgb` with alpha 0.

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
    // x: footprint on (1); y: outline width (px).
    foot: vec4<f32>,
    // rgb: the footprint's colour; a: its opacity at full level.
    foot_color: vec4<f32>,
    // x: the fields marked.
    fields: vec4<f32>,
    // w: the volume's cap ε (m).
    volume: vec4<f32>,
    // xyz: the profile's gradient `g` (the mesh's frame); w: its constant.
    profile: vec4<f32>,
    // Four fields of three lanes: origin (world) and half-angle (rad); unit axis (world) and reach
    // (m); x: how far its front has run (m), negative before it is thrown, y: its directions, z: its
    // level in 0..1.
    prints: array<vec4<f32>, 12>,
    // Four fields of 256 lanes: a recorded direction (world, unit) and its reach (m) with slack.
    rays: array<vec4<f32>, 1024>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> look: LookParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var hull_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var part_mask: texture_2d<f32>;

struct LookVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    // The mesh's origin (world).
    @location(3) origin: vec3<f32>,
    // The profile's gradient, world.
    @location(4) gradient: vec3<f32>,
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
    out.origin = (world_from_local * vec4(0.0, 0.0, 0.0, 1.0)).xyz;
    let g = look.profile.xyz;
    let turned = (world_from_local * vec4(g, 0.0)).xyz;
    out.gradient = select(vec3(0.0), normalize(turned) * length(g), dot(g, g) > 0.0);
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

fn mask_at(tex: texture_2d<f32>, p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(tex));
    return textureLoad(tex, clamp(p, vec2(0), size - 1), 0);
}

// The footprint's outline at world point `p` (normal `n`; its change across a pixel `dpx`, `dpy`):
// the level of the strongest field whose outline passes there, 0 where none does.
fn footprint(p: vec3<f32>, n: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>) -> f32 {
    if look.foot.x < 0.5 {
        return 0.0;
    }
    var mark = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        if f32(i) >= look.fields.x {
            break;
        }
        let field = i;
        let at = look.prints[3 * field];
        let along = look.prints[3 * field + 1];
        let d = p - at.xyz;
        let r = length(d);
        if r < 1e-4 || r > min(along.w, look.prints[3 * field + 2].x) || dot(n, -d) <= 0.0 {
            continue;
        }
        let dir = d / r;
        var best = -2.0;
        var reach = along.w;
        for (var j = 0; j < 256; j = j + 1) {
            if f32(j) >= look.prints[3 * field + 2].y {
                break;
            }
            let ray = look.rays[256 * field + j];
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
        // The angle's gradient in space, (c d/r - axis) / (r sin θ), across the pixel.
        let g = (c * dir - along.xyz) / max(r * sqrt(max(1.0 - c * c, 1e-8)), 1e-6);
        let per_px = abs(dot(g, dpx)) + abs(dot(g, dpy));
        if abs(angle - at.w) < 0.5 * look.foot.y * per_px {
            mark = max(mark, look.prints[3 * field + 2].z);
        }
    }
    return mark;
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
fn fragment(in: LookVertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
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
        let mark = footprint(in.world_position.xyz, n, dpx, dpy) * look.foot_color.a;
        if nearest > 0.0 && dist > nearest * 1.002 + 0.003 {
            if mark <= 0.0 {
                discard;
            }
            return vec4(look.foot_color.rgb, mark * fade);
        }
        var rgb = look.color.rgb * shading;
        var a = look.color.a * mix(look.shade.x, 1.0, pow(1.0 - facing, look.shade.z));
        if edge > 0.75 {
            rgb = look.edge.rgb;
            a = max(a, look.edge.a);
        }
        rgb = mix(rgb, look.foot_color.rgb, mark);
        a = max(a, mark);
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
        let mark = footprint(in.world_position.xyz, n, dpx, dpy) * look.foot_color.a;
        return vec4(mix(rgb, look.foot_color.rgb, mark), 1.0);
    }
    if m == 4 {
        // The reach mesh is wound outward, so a back face is where the eye ray leaves it.
        let along = -v;
        let exiting = !is_front;
        let a = view.world_position - in.origin;
        let b = dot(a, along);
        let eps = look.volume.w;
        let near = max(dot(a, a) - b * b, 0.0);
        let h = sqrt(max(near + eps * eps, 1e-12));
        let column = (atan((dist + b) / h) - atan(b / h)) / h;
        // ∫ g·x / (r (r² + ε²)) ds, with t = s + b, r² = t² + near and g·x = P + Q t:
        // P/(hε)·atanh(ε t / (h r)) + Q/ε·atan(r / ε).
        let q = dot(in.gradient, along);
        let p = dot(in.gradient, a) - b * q;
        let t1 = dist + b;
        let r0 = max(sqrt(b * b + near), 1e-9);
        let r1 = max(sqrt(t1 * t1 + near), 1e-9);
        let limit = 1.0 - 1e-6;
        let across = atanh(clamp(eps * t1 / (h * r1), -limit, limit))
            - atanh(clamp(eps * b / (h * r0), -limit, limit));
        let lean = p * across / (h * eps) + q * (atan(r1 / eps) - atan(r0 / eps)) / eps;
        let density = look.profile.w * column + lean;
        let signed = select(-density, density, exiting);
        return vec4(look.color.rgb * look.color.a * signed * fade, 0.0);
    }
    discard;
    return vec4(0.0);
}
