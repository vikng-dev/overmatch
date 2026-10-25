// The pen cam's corner look (`src/damage_cam/pen_cam/look.rs`, `LookMaterial`): one material, its
// drawing chosen per instance by `mode.x`.
//
//   0 mask        rgb: view-space normal (0..1), a: the eye depth (m, `eye_depth`). Opaque. The
//                 hull and the part masks are both drawn in it.
//   2 base        one hueless surface, drawn only where it is its mask's nearest surface: a body
//                 surface (`mode.z = 0`) the hull mask's, and nowhere the part mask holds a scene
//                 surface; a component (`mode.z = 1`) the part mask's. Half-Lambert shading with
//                 `shade.y` the light's share, opacity `color.a` at a grazing angle falling to
//                 `shade.x` of it face-on (power `shade.z`), and that mask's depth edges in `edge`
//                 (rgb, a: opacity), a component under the fields' painting (`footprint`). Each
//                 layer is one surface deep: a surface that is not its mask's nearest is discarded.
//   3 state       a component on the state scale, opaque: below its fill level `color` (red, or
//                 charcoal under an `edge`-coloured diagonal hatch when `mode.w = 1`, period
//                 `hatch.x` px, `hatch.y` of it hatched), above it `rest` (the untouched grey), a
//                 line of `edge` `span.w` px wide at the level, all half-Lambert shaded with
//                 `shade.y` the light's share. The level rises along `state.xyz` (world up, unit)
//                 from `span.x` to `span.y` (world m) by `state.w` (the share lost) times the fade;
//                 with `span.z = 0` (no known heights) the fill covers the mesh once the fade is on.
//                 The fields' painting (`footprint`) lies over the fill.
//   4 volume      the record's fields as one volume (`volume`), drawn on the back faces of a box
//                 about every field's reach, whatever stands in front. The eye ray's stretch inside
//                 each field (`stretch`: its ball out to its front or its longest reach, the
//                 nearer, and a cap's cone), clipped to the cut's kept side and, with
//                 `volume.w = 1`, short of the pixel's nearest scene surface in the part mask, is
//                 cut at every stretch's ends, and each piece between two adjacent ends is marched
//                 back toward the eye (`march`): samples of the fields' summed intensity
//                 (`intensity`) no farther apart than the finest spacing (`spacing`: its ball
//                 across over `VOLUME_STEPS`) of the fields whose stretches hold the piece, so
//                 every field is sampled at its own spacing wherever it lies, each threshold
//                 crossed between two samples placed by `VOLUME_REFINE` halvings
//                 (`step_over`). Each piece in a band takes the band's colour (`knockout`,
//                 `reduced`: rgb) at opacity `1 − exp(−length / w)`, laid over what lies behind it
//                 (`over`); the pixel takes the result at `volume.x` times its opacity, times the
//                 fade.
//
// The fields are painted on the components alone (`footprint`, `foot.x > 0`), in the bands of
// their summed intensity at opacity `foot.x`, each band's edge a pixel wide; never on the body's
// surfaces. A hard component (`foot.z = 0`) is painted where a field meets it: a surface point
// facing the field's origin, on its support, behind its front and lit within each texel's reach,
// its tolerance and `foot.y` (m) past it (`picture_at`). A crew station (`foot.z = 1`), a witness
// the field passes through, is painted wherever the volume holds the field: no facing test and no
// slack.
//
// In modes 0, 2 and 3, `mode.y = 1` discards what lies on `plane`'s normal side (the slice's cut);
// the volume clips its march to the kept side.
//
// Draw order (design doc §5, "Draw order"; `look.rs`, module doc): the body draws behind the
// scene, the components, the round's model and the fields, ordered by depth among themselves; the
// part mask holds the scene's surfaces, the components' and the round's. A body surface is
// discarded wherever the part mask holds a scene surface, and the volume is drawn after the body
// and over it; a component is opaque and depth-tested, as is the round in its own material, and
// the volume's march ends at the nearest scene surface. Every depth read against a mask is the
// eye depth (`eye_depth`): the distance from the eye in a perspective view, the distance along
// the camera's forward from its plane in an orthographic one, the masks' own cameras sharing the
// viewer's pose and lens.
//
// The bands (design doc §5, "Potency colour"; `potency::Band`): a point's intensity is the sum of
// every field's there (`intensity`), a field's `e w m / r²`, `e` the emission its strength `S`
// gives, `w` its law's weight along the point's direction (`law`) and `m` the share its depth
// picture lights at the point (`picture_at`), where the point lies on its support (from its inner
// edge to its half-angle off its axis) and within its front, and nothing elsewhere. Knockout at
// and above `bands.y` (`T_mid`), reduced from `bands.x` (`T_floor`) up to it, nothing below. No
// lane the bands are read from carries a display gain.
//
// The depth pictures (`picture::Picture`, `picture::Pictures`; `pen_cam_picture.wgsl` renders
// them): field `f`'s, the reach (record m) from its origin to the struck tank's hard bodies along
// each direction of its support, on the cube about its axis (`face_of`), each face cropped to the
// support at the field's pitch and placed in `pictures`, loaded unfiltered: each of the four
// texels about a direction tests the point against its own reach, within
// `REACH_TOLERANCE_DIAGONALS` texel diagonals (`√2` pitches) times the point's distance past it
// and never past the farthest of the four, and the results are weighted bilinearly (`picture_at`;
// `Picture::lit` is the same read). The record wins: a field whose recorded directions all ended
// at its origin has no face and a longest reach of zero, and lights nothing, at its origin
// included (`field_at`).
//
// Output: straight `rgb, a`.

#import bevy_pbr::{forward_io::Vertex, mesh_functions, mesh_view_bindings::view}

struct LookParams {
    color: vec4<f32>,
    // rgb: the edge or line colour; a: its opacity.
    edge: vec4<f32>,
    // x: face-on opacity share, y: the light's share of the shading, z: fresnel power, w: fade.
    shade: vec4<f32>,
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
    // The fields' painting (`footprint`): x its opacity (0: none), y how far (m) past each texel's
    // reach a surface counts as met, z the witness rule (1) or the hard one (0).
    foot: vec4<f32>,
    // x: the floor threshold `T_floor`, y: the mid threshold `T_mid` (HP points per m²).
    bands: vec4<f32>,
    // rgb: the knockout band's colour; w: its extinction length (m).
    knockout: vec4<f32>,
    // rgb: the reduced band's colour; w: its extinction length (m).
    reduced: vec4<f32>,
    // The volume: x its opacity at most, w bounded by the part mask's nearest scene surface (1).
    volume: vec4<f32>,
    // x: the fields drawn.
    fields: vec4<f32>,
    // `FIELDS` fields of `PRINT_LANES` lanes: origin (world) and half-angle (rad); unit axis
    // (world) and how far its front has run (m), negative before it is thrown; the picture's `u`
    // (world, unit) and the support's inner edge (rad, 0 on a cap and the sphere); the picture's
    // `v` (world, unit) and its pitch (rad a texel); the emission its strength `S` gives
    // (`ρ S / Ω`, HP points per sr, world), the world's metres per record metre, and the longest
    // reach (m); its law (`law`): the kind of its terms, their two numbers and its strata `N`; and
    // the picture's six faces, two lanes each: the crop's tangent corner `x0, y1` and its size
    // (texels), and its place in `pictures` (texels).
    prints: array<vec4<f32>, 72>,
}

// `look::FIELDS` and the lanes a field takes.
const FIELDS: i32 = 4;
const PRINT_LANES: i32 = 18;
// `look::VOLUME_STEPS`, the steps across a field's ball and the most along a piece of the march,
// and `look::VOLUME_REFINE`, the halvings that place a threshold's crossing between two samples.
const VOLUME_STEPS: i32 = 48;
const VOLUME_REFINE: i32 = 8;
// `picture::REACH_TOLERANCE_DIAGONALS`, how far past its own reach a texel counts a point: that
// many texel diagonals (`√2` pitches) times the point's distance.
const REACH_TOLERANCE_DIAGONALS: f32 = 2.0;
const HALF_PI: f32 = 1.57079633;
const PI: f32 = 3.14159265;
const SQRT_2: f32 = 1.41421356;

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> look: LookParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var hull_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var part_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var pictures: texture_2d<f32>;

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
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(vertex.position, 1.0));
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

// A direction on the cube about a field's axis: its face (`picture::FRAMES`) and its tangent there.
struct OnFace {
    face: i32,
    t: vec2<f32>,
}

// The face unit `local` (across `u` and `v`, along the axis) lies on, and its tangent there:
// `picture::face_of`.
fn face_of(local: vec3<f32>) -> OnFace {
    let a = abs(local);
    if a.z >= a.x && a.z >= a.y {
        if local.z >= 0.0 {
            return OnFace(0, vec2(local.x, -local.y) / a.z);
        }
        return OnFace(1, vec2(local.x, local.y) / a.z);
    }
    if a.x >= a.y {
        if local.x >= 0.0 {
            return OnFace(2, vec2(local.z, local.y) / a.x);
        }
        return OnFace(3, vec2(-local.z, local.y) / a.x);
    }
    if local.y >= 0.0 {
        return OnFace(4, vec2(local.x, local.z) / a.y);
    }
    return OnFace(5, vec2(local.x, -local.z) / a.y);
}

// The share (0..1) of field `f` lit along unit direction `dir` (world) at `r` world m from its
// origin, `scale` world m per record m: on the direction's face of the field's picture
// (`face_of`), each of the four texels about it in the face's crop, which the clamp holds inside
// the crop, lights where `r` lies within its own reach, and the four are weighted bilinearly. A
// texel's reach counts out to its tolerance past it (`REACH_TOLERANCE_DIAGONALS` texel diagonals
// times `r`, never past the farthest of the four), and `slack` world m past that. A reach is
// compared per texel and never interpolated, so a shadow's depth is a texel's own: across a
// silhouette a point behind the blocker, past the tolerance, gets the open texels' share alone,
// the same at every depth out to the wall. An empty face lights nothing. `Picture::lit`.
fn picture_at(f: i32, dir: vec3<f32>, r: f32, scale: f32, slack: f32) -> f32 {
    let base = PRINT_LANES * f;
    let frame_v = look.prints[base + 3];
    let local = vec3(
        dot(dir, look.prints[base + 2].xyz),
        dot(dir, frame_v.xyz),
        dot(dir, look.prints[base + 1].xyz),
    );
    let on = face_of(local);
    let crop = look.prints[base + 6 + 2 * on.face];
    let place = look.prints[base + 7 + 2 * on.face].xy;
    if crop.z < 2.0 || crop.w < 2.0 {
        return 0.0;
    }
    let pitch = frame_v.w;
    let g = vec2(on.t.x - crop.x, crop.y - on.t.y) / pitch - 0.5;
    let low = clamp(floor(g), vec2(0.0), crop.zw - 2.0);
    let t = clamp(g - low, vec2(0.0), vec2(1.0));
    let at = vec2<i32>(place + low);
    let t00 = textureLoad(pictures, at, 0).x;
    let t10 = textureLoad(pictures, at + vec2(1, 0), 0).x;
    let t01 = textureLoad(pictures, at + vec2(0, 1), 0).x;
    let t11 = textureLoad(pictures, at + vec2(1, 1), 0).x;
    let reach = vec4(t00, t10, t01, t11) * scale;
    let farthest = max(max(reach.x, reach.y), max(reach.z, reach.w));
    let tolerance = REACH_TOLERANCE_DIAGONALS * SQRT_2 * pitch * r;
    let met = min(vec4(farthest), reach + vec4(tolerance)) + vec4(slack);
    let lit = select(vec4(0.0), vec4(1.0), vec4(r) <= met);
    return mix(mix(lit.x, lit.y, t.x), mix(lit.z, lit.w, t.x), t.y);
}

// The ball's cap's share of the emission inside depth `t`, `λ²` `l2`: `1 − μ D³`,
// `D² = 1 − λ² t(2 − t)`, held at zero past the rim. `look::cap_share`.
fn cap_share(l2: f32, t: f32) -> f32 {
    let d = sqrt(max(1.0 - l2 * (t * (2.0 - t)), 0.0));
    return 1.0 - (1.0 - t) * (d * d * d);
}

// `(g, D)` of the ball over the whole sphere at depth `t`, ratio `lambda`, `c = 1 − λ²`:
// `D = √(c + λ²μ²)`, `g = λμ + D`, behind the plate `c / (D − λμ)`.
fn sphere_reach(lambda: f32, c: f32, t: f32) -> vec2<f32> {
    let mu = 1.0 - t;
    let d = sqrt(c + lambda * mu * (lambda * mu));
    if mu >= 0.0 {
        return vec2(lambda * mu + d, d);
    }
    return vec2(c / (d - lambda * mu), d);
}

// The ball's share over the whole sphere between depths `a ≤ b`, ratio `lambda` below 1:
// `(b − a)(g_a + g_b)²(g_a² + g_b² + 2c) / (16(D_a + D_b))` (`sphere_reach`). `look::sphere_share`.
fn sphere_share(lambda: f32, a: f32, b: f32) -> f32 {
    let c = (1.0 - lambda) * (1.0 + lambda);
    let at_a = sphere_reach(lambda, c, a);
    let at_b = sphere_reach(lambda, c, b);
    let sum = at_a.x + at_b.x;
    return (b - a) * (sum * sum) * (at_a.x * at_a.x + at_b.x * at_b.x + 2.0 * c)
        / (16.0 * (at_a.y + at_b.y));
}

// Field `f`'s law's weight along unit direction `dir` (world), its terms in lane 5 (x the kind: 1
// the ball's cap, `λ²` and depth `w` in y and z; 2 the ball over the whole sphere, `λ` in y; 3 the
// ring's band, its depths in y and z; w the strata `N`): at depth `t = ½|dir − axis|²` held to the
// support, the law's weight averaged over the window one stratum wide about `t`,
// `(t_hi − t_lo)/2 · share(a, b)/(b − a)`; the band's ½ throughout. At a stratum's centre it is the
// record's `f_k`. The caller tests the support. `look::law`.
fn law(f: i32, dir: vec3<f32>) -> f32 {
    let base = PRINT_LANES * f;
    let terms = look.prints[base + 5];
    let kind = i32(terms.x + 0.5);
    if (kind < 1 || kind > 3) || terms.w < 1.0 {
        return 0.0;
    }
    if kind == 3 {
        return 0.5;
    }
    let hi = select(2.0, terms.z, kind == 1);
    let off = dir - look.prints[base + 1].xyz;
    let t = clamp(0.5 * dot(off, off), 0.0, hi);
    let half = 0.5 * hi / terms.w;
    let a = max(t - half, 0.0);
    let b = min(t + half, hi);
    if b <= a {
        return 0.0;
    }
    var share = sphere_share(terms.y, a, b);
    if kind == 1 {
        share = cap_share(terms.y, b) - cap_share(terms.y, a);
    }
    return 0.5 * hi * share / (b - a);
}

// Field `f`'s intensity at world point `p` (HP points per m²): `e w m / r²` on its support and
// within its front, `w` its law's weight along `p`'s direction (`law`) and `m` the share its
// picture lights at `p` (`picture_at`), 0 elsewhere, past its longest reach included; at its
// origin, once thrown, unbounded, unless the record ended every direction of it there (its longest
// reach zero): the record wins.
fn field_at(f: i32, p: vec3<f32>) -> f32 {
    let base = PRINT_LANES * f;
    let at = look.prints[base];
    let along = look.prints[base + 1];
    let scale = look.prints[base + 4];
    let d = p - at.xyz;
    let r = length(d);
    if r < 1e-5 {
        return select(0.0, 1e30, along.w > 0.0 && scale.z > 0.0);
    }
    if r > min(along.w, scale.z) {
        return 0.0;
    }
    let dir = d / r;
    let c = clamp(dot(dir, along.xyz), -1.0, 1.0);
    if c > cos(look.prints[base + 2].w) || c < cos(at.w) {
        return 0.0;
    }
    return picture_at(f, dir, r, scale.y, 0.0) * law(f, dir) * scale.x / (r * r);
}

// The fields' summed intensity at world point `p` (HP points per m²): doses add, so the bands
// classify the sum. `look::intensity`.
fn intensity(p: vec3<f32>) -> f32 {
    var sum = 0.0;
    for (var f = 0; f < FIELDS; f = f + 1) {
        if f32(f) >= look.fields.x {
            break;
        }
        sum = sum + field_at(f, p);
    }
    return sum;
}

// A band's edge one pixel wide: `smoothstep` over `width` centred on `edge`, `potency::soft_step`.
fn soft_step(edge: f32, width: f32, x: f32) -> f32 {
    let half = 0.5 * max(width, 1e-6);
    return smoothstep(edge - half, edge + half, x);
}

// How much of the pixel lies on the positive side of signed distance `s`: its zero crossing one
// pixel wide. Called in uniform control flow: it takes a derivative.
fn inside(s: f32) -> f32 {
    return clamp(s / max(fwidth(s), 1e-7) + 0.5, 0.0, 1.0);
}

// The fields' painting at world point `p` on a component's surface of unit normal `n` (world):
// the presence over the pixel of the floor (x: the reduced and the knockout bands together) and
// of the knockout band (y). A field covers the point where it lies on the field's support, behind
// its front and within its longest reach and `foot.y` past it (as the volume, `field_at`, holds
// nothing past the longest reach) and, under the hard rule (`foot.z = 0`), faces the field's
// origin, each edge a pixel wide, at its law's weight there (`law`) lit as its picture lights it
// `foot.y` past each texel's reach (`picture_at`); the witness rule (`foot.z = 1`) asks no facing.
// The covered fields' intensities add, and each band's edge is a smoothstep a pixel wide in `log2`
// of the sum (`soft_step`). `look::painting`.
// Called in uniform control flow: it takes derivatives.
fn footprint(p: vec3<f32>, n: vec3<f32>) -> vec2<f32> {
    var sum = 0.0;
    for (var f = 0; f < FIELDS; f = f + 1) {
        if f32(f) >= look.fields.x {
            break;
        }
        let base = PRINT_LANES * f;
        let at = look.prints[base];
        let along = look.prints[base + 1];
        let inner = look.prints[base + 2].w;
        let scale = look.prints[base + 4];
        let d = p - at.xyz;
        let r = max(length(d), 1e-5);
        let dir = d / r;
        let theta = acos(clamp(dot(dir, along.xyz), -1.0, 1.0));
        // Each edge as a signed distance, positive inside: short of the front and of the longest
        // reach and the slack past it (m), off the rim and short of the inner edge (m across the
        // surface's direction, `r` times the angle), and the facing (the cosine of the origin off
        // the normal, 1 under the witness rule).
        let rim = select(1e6, r * (at.w - theta), at.w < PI);
        let hole = select(1e6, r * (theta - inner), inner > 0.0);
        let facing = select(dot(n, -dir), 1.0, look.foot.z > 0.5);
        let reach = min(along.w, scale.z + look.foot.y);
        let cover = inside(reach - r) * inside(rim) * inside(hole) * inside(facing);
        let lit = picture_at(f, dir, r, scale.y, look.foot.y) * law(f, dir);
        sum = sum + cover * lit * scale.x / (r * r);
    }
    // Below a 4096th of the floor the sum is nothing: its log stays finite for the derivative.
    let level = log2(max(sum, look.bands.x / 4096.0));
    let width = fwidth(level);
    return vec2(
        soft_step(log2(look.bands.x), width, level),
        soft_step(log2(look.bands.y), width, level),
    );
}

// A surface's colour `rgb` under the painting `print` (`footprint`): each band's colour over it at
// its presence times `foot.x`.
fn printed(rgb: vec3<f32>, print: vec2<f32>) -> vec3<f32> {
    let bands = look.knockout.rgb * print.y + look.reduced.rgb * (print.x - print.y);
    return rgb * (1.0 - look.foot.x * print.x) + look.foot.x * bands;
}

// Field `f`'s stretch of the ray `eye + dir t` (t ≥ 0, `dir` unit): inside its ball out to its
// front or its longest reach, the nearer, and for a cap no wider than a hemisphere inside its cone
// too; empty (x ≥ y) where the ray misses it. `look::stretch`.
fn stretch(f: i32, eye: vec3<f32>, dir: vec3<f32>) -> vec2<f32> {
    let base = PRINT_LANES * f;
    let at = look.prints[base];
    let along = look.prints[base + 1];
    let none = vec2(1.0, 0.0);
    let radius = min(along.w, look.prints[base + 4].z);
    if radius <= 0.0 {
        return none;
    }
    let oc = eye - at.xyz;
    let b = dot(oc, dir);
    let h = b * b - (dot(oc, oc) - radius * radius);
    if h <= 0.0 {
        return none;
    }
    var lo = -b - sqrt(h);
    var hi = -b + sqrt(h);
    if at.w < HALF_PI && look.prints[base + 2].w <= 0.0 {
        let cos_half = cos(at.w);
        let cos2 = cos_half * cos_half;
        let dv = dot(dir, along.xyz);
        let ov = dot(oc, along.xyz);
        // The cone's own nappe: `ov + t dv ≥ 0`.
        if abs(dv) > 1e-6 {
            let t0 = -ov / dv;
            if dv > 0.0 {
                lo = max(lo, t0);
            } else {
                hi = min(hi, t0);
            }
        } else if ov < 0.0 {
            return none;
        }
        // Inside the double cone: `qa t² + qb t + qc ≥ 0`.
        let qa = dv * dv - cos2;
        let qb = 2.0 * (dv * ov - cos2 * b);
        let qc = ov * ov - cos2 * dot(oc, oc);
        let disc = qb * qb - 4.0 * qa * qc;
        if abs(qa) < 1e-7 {
            if abs(qb) > 1e-12 {
                let t1 = -qc / qb;
                if qb > 0.0 {
                    lo = max(lo, t1);
                } else {
                    hi = min(hi, t1);
                }
            } else if qc < 0.0 {
                return none;
            }
        } else if qa < 0.0 {
            if disc <= 0.0 {
                return none;
            }
            let root = sqrt(disc);
            let t1 = (-qb + root) / (2.0 * qa);
            let t2 = (-qb - root) / (2.0 * qa);
            lo = max(lo, min(t1, t2));
            hi = min(hi, max(t1, t2));
        } else if disc > 0.0 {
            let root = sqrt(disc);
            if dv > 0.0 {
                lo = max(lo, (-qb + root) / (2.0 * qa));
            } else {
                hi = min(hi, (-qb - root) / (2.0 * qa));
            }
        }
    }
    return vec2(max(lo, 0.0), hi);
}

// A piece `span` (m) long in band `band` (0 nothing, 1 reduced, 2 knockout) laid over `acc`
// (premultiplied rgb, a), nearer than all of it.
fn over(acc: vec4<f32>, span: f32, band: i32) -> vec4<f32> {
    if band == 0 || span <= 0.0 {
        return acc;
    }
    let lane = select(look.reduced, look.knockout, band == 2);
    let a = 1.0 - exp(-span / lane.w);
    return vec4(lane.rgb * a + acc.rgb * (1.0 - a), a + acc.a * (1.0 - a));
}

// Where threshold `threshold` is crossed between `t_far` and `t_near` on the ray `eye + dir t`,
// the far end above it if `far_above`, by `VOLUME_REFINE` halvings.
fn crossing(eye: vec3<f32>, dir: vec3<f32>, t_far: f32, t_near: f32, threshold: f32, far_above: bool) -> f32 {
    var a = t_far;
    var b = t_near;
    for (var j = 0; j < VOLUME_REFINE; j = j + 1) {
        let between = 0.5 * (a + b);
        if (intensity(eye + dir * between) >= threshold) == far_above {
            a = between;
        } else {
            b = between;
        }
    }
    return 0.5 * (a + b);
}

// One step of the march, from `t_far` (intensity `i_far`) back to `t_near` (`i_near`), laid over
// `acc`: cut where each threshold the two samples lie apart on is crossed (`crossing`), each piece
// in the band its side of each crossing holds. `look::march`.
fn step_over(acc: vec4<f32>, eye: vec3<f32>, dir: vec3<f32>, t_far: f32, i_far: f32, t_near: f32, i_near: f32) -> vec4<f32> {
    let f_far = i_far >= look.bands.x;
    let f_near = i_near >= look.bands.x;
    let m_far = i_far >= look.bands.y;
    let m_near = i_near >= look.bands.y;
    var c_f = t_near;
    var c_m = t_near;
    if f_far != f_near {
        c_f = crossing(eye, dir, t_far, t_near, look.bands.x, f_far);
    }
    if m_far != m_near {
        c_m = crossing(eye, dir, t_far, t_near, look.bands.y, m_far);
    }
    var points = array<f32, 4>(t_far, max(c_f, c_m), min(c_f, c_m), t_near);
    var out = acc;
    for (var k = 0; k < 3; k = k + 1) {
        let a = points[k];
        let b = points[k + 1];
        if a <= b {
            continue;
        }
        let t = 0.5 * (a + b);
        let f_in = select(f_far, f_near, f_far != f_near && t < c_f);
        let m_in = select(m_far, m_near, m_far != m_near && t < c_m);
        out = over(out, a - b, select(select(0, 1, f_in), 2, m_in));
    }
    return out;
}

// Field `f`'s spacing (m): its ball across in `VOLUME_STEPS` steps, its radius the nearer of its
// front and its longest reach, as `stretch` bounds it. `look::spacing`.
fn spacing(f: i32) -> f32 {
    let base = PRINT_LANES * f;
    return 2.0 * min(look.prints[base + 1].w, look.prints[base + 4].z) / f32(VOLUME_STEPS);
}

// The piece from `near` to `far` on the ray `eye + dir t` marched back toward the eye in equal
// steps no longer than `space`, `VOLUME_STEPS` at most, laid over `acc`. `look::march`.
fn march(acc: vec4<f32>, eye: vec3<f32>, dir: vec3<f32>, near: f32, far: f32, space: f32) -> vec4<f32> {
    let steps = i32(clamp(ceil((far - near) / max(space, 1e-6)), 1.0, f32(VOLUME_STEPS)));
    let step = (far - near) / f32(steps);
    var out = acc;
    var t_far = far;
    var i_far = intensity(eye + dir * far);
    for (var k = 1; k <= steps; k = k + 1) {
        let t_near = far - f32(k) * step;
        let i_near = intensity(eye + dir * t_near);
        out = step_over(out, eye, dir, t_far, i_far, t_near, i_near);
        t_far = t_near;
        i_far = i_near;
    }
    return out;
}

// The fields' volume seen through `exit`, a point where the eye ray leaves the box about them, at
// mask pixel `p`: each field's stretch of the ray, clipped to the cut's kept side and short of
// the nearest scene surface, cut at every stretch's ends and marched far to near, each piece at
// the finest spacing of the fields whose stretches hold it (`look::schedule`). A field's ends are
// samples and its stretch is sampled at its own spacing or finer, so a small field never falls
// between two samples of a wide one.
fn volume(exit: vec3<f32>, p: vec2<i32>, fade: f32) -> vec4<f32> {
    // Depths along the ray are eye depths: `eye + dir t` lies at eye depth `t`.
    let dir = eye_ray(exit);
    let eye = exit - dir * eye_depth(exit);
    var near = 0.0;
    var far = 1e30;
    if look.volume.w > 0.5 {
        let bound = mask_at(part_mask, p).a;
        if bound > 0.0 {
            far = bound;
        }
    }
    if look.mode.y > 0.5 {
        // The kept side: `plane · (eye + dir t) ≤ w`.
        let g0 = dot(look.plane.xyz, eye) - look.plane.w;
        let gd = dot(look.plane.xyz, dir);
        if abs(gd) < 1e-8 {
            if g0 > 0.0 {
                discard;
            }
        } else if gd > 0.0 {
            far = min(far, -g0 / gd);
        } else {
            near = max(near, -g0 / gd);
        }
    }
    // Each field's stretch (empty, `lo > hi`, where the ray misses it) and every stretch's ends in
    // order.
    var lo = array<f32, 4>(1.0, 1.0, 1.0, 1.0);
    var hi = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
    var ends = array<f32, 8>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    var count = 0;
    for (var f = 0; f < FIELDS; f = f + 1) {
        if f32(f) >= look.fields.x {
            break;
        }
        let s = stretch(f, eye, dir);
        let a = max(s.x, near);
        let b = min(s.y, far);
        if a >= b {
            continue;
        }
        lo[f] = a;
        hi[f] = b;
        for (var e = 0; e < 2; e = e + 1) {
            let t = select(b, a, e == 0);
            var k = count;
            while k > 0 && ends[k - 1] > t {
                ends[k] = ends[k - 1];
                k = k - 1;
            }
            ends[k] = t;
            count = count + 1;
        }
    }
    if count == 0 {
        discard;
    }
    // Far to near, each piece between adjacent ends that a stretch holds, a sample counting every
    // field once.
    var acc = vec4(0.0);
    for (var k = count - 1; k > 0; k = k - 1) {
        let a = ends[k - 1];
        let b = ends[k];
        let mid = 0.5 * (a + b);
        var space = 1e30;
        for (var f = 0; f < FIELDS; f = f + 1) {
            if lo[f] <= mid && mid <= hi[f] {
                space = min(space, spacing(f));
            }
        }
        if a < b && space < 1e30 {
            acc = march(acc, eye, dir, a, b, space);
        }
    }
    if acc.a <= 0.0 {
        discard;
    }
    return vec4(acc.rgb / acc.a, clamp(look.volume.x * acc.a * fade, 0.0, 1.0));
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
    // The state fill's height and its change across a pixel, and the fields' painting, taken in
    // uniform control flow.
    let height = dot(in.world_position.xyz, look.state.xyz);
    let height_px = fwidth(height);
    var print = vec2(0.0);
    if look.foot.x > 0.0 {
        print = footprint(in.world_position.xyz, n);
    }
    let toward = select(-n, n, dot(n, v) >= 0.0);
    let lambert = 0.5 * (dot(toward, normalize(vec3(0.4, 1.0, 0.3))) + 1.0);
    let shading = 1.0 - look.shade.y + look.shade.y * lambert;
    // Where the masks hold this fragment: its position within its own viewport.
    let pix = frag - view.viewport.xy;
    let depth = eye_depth(in.world_position.xyz);

    if m == 4 {
        return volume(in.world_position.xyz, vec2<i32>(pix), fade);
    }
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
            // The body is behind the scene.
            if mask_at(part_mask, p).a > 0.0 {
                discard;
            }
            nearest = mask_at(hull_mask, p).a;
            edge = lines(hull_mask, pix);
        }
        if nearest > 0.0 && depth > nearest * 1.002 + 0.003 {
            discard;
        }
        var rgb = look.color.rgb * shading;
        var a = look.color.a * mix(look.shade.x, 1.0, pow(1.0 - facing, look.shade.z));
        if edge > 0.75 {
            rgb = look.edge.rgb;
            a = max(a, look.edge.a);
        }
        rgb = printed(rgb, print);
        a = max(a, look.foot.x * print.x);
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
        return vec4(printed(rgb, print), 1.0);
    }
    discard;
    return vec4(0.0);
}
