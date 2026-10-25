// A field's depth picture (`src/damage_cam/pen_cam/picture.rs`): one face of the cube about the
// field's axis, seen from its origin, drawn into that face's place in the record's picture
// atlas. The casters are the struck tank's hard bodies and the horizon box about them
// (`picture::Casters`), every triangle wound outward but the horizon's, wound inward; the
// pipeline culls the faces a line from the origin meets from behind (`picture::CASTER_CULL`), so
// a line from inside a body leaves it unstopped. Each texel holds the radial distance (record m)
// from the origin to the nearest face it meets head-on along the texel's centre direction: the
// reach the look tests a point against (`pen_cam_look.wgsl`, `picture_at`).
//
// The face's frame (record frame, unit): `right`, `up` and `forward`, `right × up = −forward`;
// `origin` the field's origin; `rect` the face's crop (x0, y0, x1, y1) in tangent units, a
// direction `forward + x·right + y·up`. The clip puts x0..x1 and y0..y1 across the viewport and the
// near plane `NEAR` m along `forward`; the depth is reverse (`NEAR / z`, nearer greater).

struct Face {
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    origin: vec4<f32>,
    rect: vec4<f32>,
}

@group(0) @binding(0) var<uniform> face: Face;

// `picture::NEAR_M`.
const NEAR: f32 = 1e-4;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
}

@vertex
fn vertex(@location(0) position: vec3<f32>) -> Out {
    let q = position - face.origin.xyz;
    let x = dot(q, face.right.xyz);
    let y = dot(q, face.up.xyz);
    let z = dot(q, face.forward.xyz);
    let r = face.rect;
    var out: Out;
    out.clip = vec4(
        (2.0 * x - (r.x + r.z) * z) / (r.z - r.x),
        (2.0 * y - (r.y + r.w) * z) / (r.w - r.y),
        NEAR,
        z,
    );
    out.world = position;
    return out;
}

@fragment
fn fragment(in: Out) -> @location(0) vec4<f32> {
    return vec4(length(in.world - face.origin.xyz), 0.0, 0.0, 1.0);
}
