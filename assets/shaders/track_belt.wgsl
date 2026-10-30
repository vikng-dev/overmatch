// The belt's placement: one rigid shoe per material link, read from the belt's block of the shared
// table.
//
// Every material that draws a belt mesh (`track::belt_draw`) binds a table page at binding 100 of
// its material group and imports this file. A belt's block starts at the record its `MeshTag`
// names: four header records (the mesh -> canonical rotation's columns in `xyz`, then the canonical
// origin in `xyz` and the shoe's lateral centre in `w`), then one span per material link. The
// placement is the CPU placer's (`track::link_view::link_transform`): a mesh-space point `p` of
// link `l` lands at `swing(correction * (p - origin), dir) + anchor`, where the span `(a, b)` is
// the link's two pin joints in side-plane `(z, y)`, `anchor = (lateral_x, mid.y, mid.x)` with
// `mid = (a + b) / 2`, and the swing is the rotation about the belt's lateral axis that carries
// canonical `+z` onto the unit chord `dir = (b - a) / |b - a|`. A shoe is never stretched to its
// chord or bent along the route.

#import bevy_pbr::mesh_functions::get_tag

// One table page: every belt block on it, each record a `vec4`.
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<storage, read> belt_table: array<vec4<f32>>;

// One belt's placement constants, and where its spans start.
struct Belt {
    spans: u32,
    correction: mat3x3<f32>,
    origin: vec3<f32>,
    lateral_x: f32,
}

// The belt the mesh instance `instance_index` draws.
fn belt_at(instance_index: u32) -> Belt {
    let block = get_tag(instance_index);
    let placement = belt_table[block + 3u];
    return Belt(
        block + 4u,
        mat3x3<f32>(belt_table[block].xyz, belt_table[block + 1u].xyz, belt_table[block + 2u].xyz),
        placement.xyz,
        placement.w,
    );
}

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

// A shoe vertex at mesh-space `position`, placed as link `link` of `belt` in the belt's frame.
fn link_position(belt: Belt, link: u32, position: vec3<f32>) -> vec3<f32> {
    let span = belt_table[belt.spans + link];
    let mid = (span.xy + span.zw) * 0.5;
    let q = belt.correction * (position - belt.origin);
    return swing(q, link_dir(span)) + vec3(belt.lateral_x, mid.y, mid.x);
}

// A mesh-space direction (a normal or a tangent) of link `link` of `belt`, rotated into the belt's
// frame.
fn link_direction(belt: Belt, link: u32, direction: vec3<f32>) -> vec3<f32> {
    return swing(belt.correction * direction, link_dir(belt_table[belt.spans + link]));
}
