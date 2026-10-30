// The belt's placement: one rigid shoe per material link, read from the belt's frame table.
//
// Every material that draws a belt mesh (`track::belt_draw`) binds the same two resources at
// bindings 100 and 101 of its material group and imports this file. The placement is the CPU
// placer's (`track::link_view::link_transform`): a mesh-space point `p` of link `l` lands at
// `swing(correction * (p - origin), dir) + anchor`, where the span `(a, b)` is the link's two pin
// joints in side-plane `(z, y)`, `anchor = (lateral_x, mid.y, mid.x)` with `mid = (a + b) / 2`, and
// the swing is the rotation about the belt's lateral axis that carries canonical `+z` onto the unit
// chord `dir = (b - a) / |b - a|`. A shoe is never stretched to its chord or bent along the route.

struct BeltParams {
    // Mesh -> canonical rotation of this side's template.
    correction: mat3x3<f32>,
    // The canonical origin in mesh space.
    origin: vec3<f32>,
    // The shoe's lateral centre in the belt's frame (m).
    lateral_x: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> belt: BeltParams;
// One record per material link: the span's joints `(a.z, a.y, b.z, b.y)`.
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var<storage, read> belt_links: array<vec4<f32>>;

// A span's unit chord in side-plane `(z, y)`; a zero chord keeps canonical `+z` on the belt's `+z`.
fn link_dir(span: vec4<f32>) -> vec2<f32> {
    let chord = span.zw - span.xy;
    let span_length = length(chord);
    if span_length > 0.0 {
        return chord / span_length;
    }
    return vec2(1.0, 0.0);
}

// The rotation about the belt's lateral axis by `-atan2(dir.y, dir.x)`.
fn swing(q: vec3<f32>, dir: vec2<f32>) -> vec3<f32> {
    return vec3(q.x, q.y * dir.x + q.z * dir.y, q.z * dir.x - q.y * dir.y);
}

// A shoe vertex at mesh-space `position`, placed as link `link` in the belt's frame.
fn link_position(link: u32, position: vec3<f32>) -> vec3<f32> {
    let span = belt_links[link];
    let mid = (span.xy + span.zw) * 0.5;
    let q = belt.correction * (position - belt.origin);
    return swing(q, link_dir(span)) + vec3(belt.lateral_x, mid.y, mid.x);
}

// A mesh-space direction (a normal or a tangent) of link `link`, rotated into the belt's frame.
fn link_direction(link: u32, direction: vec3<f32>) -> vec3<f32> {
    return swing(belt.correction * direction, link_dir(belt_links[link]));
}
