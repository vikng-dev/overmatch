// A building's holes, drawn without touching its mesh (`buildings::view::hole_table`): the surface
// inside a hole is skipped, and the eye ray through it either leaves through the far side of the
// chunk (the hole is seen through) or meets the hole's inner wall inside the chunk. The main pass
// and the prepass (shadow casting) both ask `hole_at`, each with its own view's ray, so a hole
// lets through the light it lets the eye through.
//
// THE TABLE (binding 105 of the material group, one `vec4` per record, mesh-local metres): record
// `c` for law chunk `c` is its header `(first, holes, planes, 0)`, then from record `first` its
// holes, two records each (`(from, radius)`, `(to, 0)`: the capsule of radius `radius` about the
// segment), then its hull's planes (`(n, o)`: a point `x` is inside when `n · x ≤ o`). A face
// carries `c + 1` in its second UV's `y`, 0 on a whole element, which holds no hole.
//
// The depth written stays the face's own: an inner wall is shaded where it is but sorted where
// the face is, so a body inside the hole's volume hides behind the face it went through.

#import bevy_pbr::{mesh_functions, mesh_view_bindings::view}

@group(#{MATERIAL_BIND_GROUP}) @binding(105) var<storage, read> hole_table: array<vec4<f32>>;

// `breakage::HOLES_PER_CHUNK`, the most holes a chunk's header names (a test pins the two equal).
const HOLES_PER_CHUNK: u32 = 16u;

// How far past a plane, or past a span's end, still counts as on it, m.
const ON_SURFACE_M: f32 = 1e-3;

const NO_HOLE: u32 = 0u;
const SEE_THROUGH: u32 = 1u;
const INNER_WALL: u32 = 2u;

struct HoleHit {
    // NO_HOLE, SEE_THROUGH or INNER_WALL.
    kind: u32,
    // INNER_WALL only: the wall's point and its unit normal, facing into the hole, world frame.
    at: vec3<f32>,
    normal: vec3<f32>,
}

// The span `(t_in, t_out)` of the ray `p + t d` inside the capsule of radius `r` about `a`–`b`;
// `t_in > t_out` when the ray misses it. The capsule is convex, so its span is the hull of its
// parts' spans: the two end balls and the cylinder between the end planes.
fn capsule_span(p: vec3<f32>, d: vec3<f32>, a: vec3<f32>, b: vec3<f32>, r: f32) -> vec2<f32> {
    var span = vec2(1e30, -1e30);
    let dd = dot(d, d);
    for (var cap = 0u; cap < 2u; cap++) {
        let o = p - select(a, b, cap == 1u);
        let half_b = dot(d, o);
        let disc = half_b * half_b - dd * (dot(o, o) - r * r);
        if disc >= 0.0 {
            let root = sqrt(disc);
            span = vec2(min(span.x, (-half_b - root) / dd), max(span.y, (-half_b + root) / dd));
        }
    }
    let axis = b - a;
    let length_ab = length(axis);
    if length_ab > 1e-6 {
        let u = axis / length_ab;
        let o = p - a;
        let d_perp = d - dot(d, u) * u;
        let o_perp = o - dot(o, u) * u;
        let qa = dot(d_perp, d_perp);
        let qb = dot(d_perp, o_perp);
        let qc = dot(o_perp, o_perp) - r * r;
        let disc = qb * qb - qa * qc;
        if qa > 1e-12 && disc >= 0.0 {
            let root = sqrt(disc);
            var t0 = (-qb - root) / qa;
            var t1 = (-qb + root) / qa;
            // Clip to the end planes, where the axial coordinate `s(t) = s0 + t ds` is in [0, L].
            let s0 = dot(o, u);
            let ds = dot(d, u);
            if abs(ds) > 1e-12 {
                let ta = (0.0 - s0) / ds;
                let tb = (length_ab - s0) / ds;
                t0 = max(t0, min(ta, tb));
                t1 = min(t1, max(ta, tb));
            } else if s0 < 0.0 || s0 > length_ab {
                t1 = t0 - 1.0;
            }
            if t0 <= t1 {
                span = vec2(min(span.x, t0), max(span.y, t1));
            }
        }
    }
    return span;
}

// The closest point to `x` on the segment `a`–`b`.
fn closest_on_segment(x: vec3<f32>, a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let axis = b - a;
    let s = clamp(dot(x - a, axis) / max(dot(axis, axis), 1e-12), 0.0, 1.0);
    return a + s * axis;
}

// What the view's ray shows at `world_position`, a fragment of the face tagged `tag` (module doc)
// on mesh instance `instance_index`.
fn hole_at(world_position: vec3<f32>, tag: f32, instance_index: u32) -> HoleHit {
    var hit: HoleHit;
    hit.kind = NO_HOLE;
    let chunk = u32(round(tag));
    if chunk == 0u {
        return hit;
    }
    let header = hole_table[chunk - 1u];
    let first = u32(header.x);
    let holes = min(u32(header.y), HOLES_PER_CHUNK);
    let planes = u32(header.z);
    if holes == 0u {
        return hit;
    }
    let local_from_world = mesh_functions::get_local_from_world(instance_index);
    let p = (local_from_world * vec4(world_position, 1.0)).xyz;

    var inside = false;
    for (var k = 0u; k < holes; k++) {
        let start = hole_table[first + 2u * k];
        let end = hole_table[first + 2u * k + 1u].xyz;
        inside = inside || distance(p, closest_on_segment(p, start.xyz, end)) < start.w;
    }
    if !inside {
        return hit;
    }

    // The view's ray through the fragment, away from the eye (`pbr_functions::calculate_view`,
    // reversed): one direction on an orthographic view (a directional light's), from the eye on a
    // perspective one.
    var ray: vec3<f32>;
    if view.clip_from_view[3].w == 1.0 {
        ray = -vec3(view.clip_from_world[0].z, view.clip_from_world[1].z, view.clip_from_world[2].z);
    } else {
        ray = world_position - view.world_position.xyz;
    }
    // A unit ray in the mesh's frame, so the walk's `t` is in its metres.
    let d = normalize((local_from_world * vec4(ray, 0.0)).xyz);

    var t_in: array<f32, HOLES_PER_CHUNK>;
    var t_out: array<f32, HOLES_PER_CHUNK>;
    for (var k = 0u; k < holes; k++) {
        let start = hole_table[first + 2u * k];
        let end = hole_table[first + 2u * k + 1u].xyz;
        let span = capsule_span(p, d, start.xyz, end, start.w);
        t_in[k] = span.x;
        t_out[k] = span.y;
    }
    // Where the ray leaves the holes' union: walk from the face to the far end of every span that
    // holds the walk's point, until none does. Each pass that moves takes one more span; `holes`
    // passes take them all.
    var t = 0.0;
    var last = 0u;
    for (var sweep = 0u; sweep < holes; sweep++) {
        var moved = false;
        for (var k = 0u; k < holes; k++) {
            if t_in[k] <= t + ON_SURFACE_M && t_out[k] > t {
                t = t_out[k];
                last = k;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    let leave = p + t * d;

    hit.kind = INNER_WALL;
    for (var k = 0u; k < planes; k++) {
        let plane = hole_table[first + 2u * holes + k];
        if dot(plane.xyz, leave) > plane.w + ON_SURFACE_M {
            hit.kind = SEE_THROUGH;
        }
    }
    if hit.kind == INNER_WALL {
        let start = hole_table[first + 2u * last];
        let end = hole_table[first + 2u * last + 1u].xyz;
        let inward = closest_on_segment(leave, start.xyz, end) - leave;
        hit.at = (mesh_functions::get_world_from_local(instance_index) * vec4(leave, 1.0)).xyz;
        hit.normal = normalize(mesh_functions::mesh_normal_local_to_world(inward, instance_index));
    }
    return hit;
}
