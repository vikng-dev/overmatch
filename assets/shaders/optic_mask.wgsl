// The gunner optic's surround: an opaque field with one circular hole punched in it — the glass.
// Placement is entirely the CPU's (`sight::reticle::optic_glass`): this only draws the disc it is
// handed. The centre is the gun's sight line reprojected, so it is NOT the node's centre except
// where the camera happens to sit on that line.

#import bevy_ui::ui_vertex_output::UiVertexOutput

struct OpticMask {
    // xy: the glass centre in node UV. z: its radius, w: the half-width of its edge feather — both
    // as fractions of the node's HEIGHT, so the hole stays a circle at any aspect ratio.
    glass: vec4<f32>,
    // What lies outside the glass, linear RGBA.
    surround: vec4<f32>,
}

@group(1) @binding(0)
var<uniform> mask: OpticMask;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    // Height units in both axes: the UV offset is stretched by the node's aspect on x alone.
    let offset = (in.uv - mask.glass.xy) * vec2<f32>(in.size.x / max(in.size.y, 1.0), 1.0);
    // A zero feather would collapse the smoothstep's interval and undefine the edge.
    let feather = max(mask.glass.w, 1e-6);
    let surround = smoothstep(mask.glass.z - feather, mask.glass.z + feather, length(offset));
    return vec4<f32>(mask.surround.rgb, mask.surround.a * surround);
}
