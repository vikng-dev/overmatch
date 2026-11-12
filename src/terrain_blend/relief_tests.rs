//! Headless checks of the shipped relief helpers, using their WGSL rather than a Rust copy.

use super::*;

fn helpers() -> String {
    let source = include_str!("../../assets/shaders/terrain_blend.wgsl");
    let start = source.find("struct TerrainBlendParams").unwrap();
    let end = source.find("@fragment").unwrap();
    source[start..end].replace("#{MATERIAL_BIND_GROUP}", "0")
}

/// The terrain's divergent marches must remain valid WGSL without implicit derivatives. Parsing
/// and validating the actual helpers catches an implicit textureSample in either march, before a
/// player selects parallax or sun shadow and pipeline creation fails.
#[test]
fn terrain_blend_relief_helpers_validate_without_implicit_derivatives() {
    let source = format!(
        "{}\n@compute @workgroup_size(1) fn check(@builtin(global_invocation_id) id: vec3<u32>) {{
            let p = vec2(f32(id.x), 1.0);
            let ground = vec4(1.0, 0.0, 0.0, 0.0);
            let depth = parallax_depth(p, vec2(20.0, 0.0), ground, 1.0, 0.15);
            let sun = sun_visibility(p, depth, vec2(1.0, 0.0), ground, 1.0);
        }}",
        helpers()
    );
    let module = wgpu::naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|err| panic!("{}", err.emit_to_string(&source)));
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("the shipped relief helpers validate");
}

/// The terrain's relief must land on the view ray on slopes, find a one-texel ridge at grazing
/// angles, fade its sun shadow with the selected reach, and never sample unrelated torus storage
/// beyond the resident window. These are numerical checks of the shipped WGSL on a surface-free
/// compute device; a Rust mirror or shader-text assertion would let a broken march pass. The fade
/// is sampled where the selected reach and fade lanes put its ends and its middle.
#[test]
#[ignore = "manual headless GPU correctness check; requires a compute adapter, never a window"]
fn terrain_blend_relief_landings_ridges_fade_and_window_are_correct() {
    // Synthetic raster: 128-square, the production texel spacing; a ridge at column 33.
    const SIDE: u32 = 128;
    let texels: Vec<u8> = (0..SIDE * SIDE)
        .flat_map(|i| if i % SIDE == 33 { [0, 0] } else { [255, 100] })
        .collect();
    let mut params = TerrainBlendParams {
        print_near: Vec4::new(0.0, 0.0, SIDE as f32 * 0.046875, 0.046875),
        print_softness: Vec4::splat(0.0015), // Fixture: 0.15 m at 100 kPa.
        print_cap: Vec4::splat(0.15),
        print_law: crate::track_prints::law_params(),
        print_store: crate::track_prints::store_params(),
        ..default()
    };
    params.print_relief = crate::track_prints::relief_params(
        crate::settings::TrackPrintRelief::SunShadow,
        crate::settings::TrackPrintRange::Far,
    );
    let values = evaluate(
        "let normal = normalize(vec3(-0.5, 1.0, 0.0));
    let into = normalize(vec3(0.8, -0.3, 0.1));
    let ray = vertical_depth_ray(into, normal, -dot(into, normal));
    result[0] = vec4(ray * 0.15, 0.0);
    let ground = vec4(1.0, 0.0, 0.0, 0.0);
    // Fixture: full-depth floor except a single raised column, 13 texels along this ray.
    let p = vec2(20.5, 64.5) * blend.print_near.w;
    result[1] = vec4(parallax_depth(p, vec2(20.0, 0.0), ground, 1.0, 0.15),
        near_texel(vec2(-0.5, p.y)), 0.0);
    let reach = blend.print_relief.y;
    let span = blend.print_relief.z;
    let fade = relief_weight(reach - 0.5 * span);
    // March out of the resident window: the surrounding unprinted ground blocks the ray.
    result[2] = vec4(sun_visibility(vec2(-1.0), 0.15 * fade, vec2(0.0), ground, fade),
        fade, relief_weight(reach), relief_weight(reach - span));
    // A uniform floor away from the ridge has no self-shadow.
    result[3] = vec4(sun_visibility(vec2(4.0, 3.0), 0.15, vec2(0.0), ground, 1.0));",
        &params,
        SIDE,
        &texels,
        4,
    );
    let offset = Vec3::from_slice(&values[..3]);
    let direction = Vec3::new(0.8, -0.3, 0.1).normalize();
    assert!(offset.cross(direction).length() < 1e-6, "on the view ray");
    assert!(
        (offset.y - 0.5 * offset.x + 0.15).abs() < 1e-6,
        "vertical depth below the sloped plane: {offset}"
    );
    assert!(
        (0.025..0.035).contains(&values[4]),
        "the one-texel ridge is the first landing, not the floor: {}",
        values[4]
    );
    assert_eq!(&values[5..7], &[0.0, 0.0], "no wrapped history off-window");
    let [half, gone, whole] = [values[9], values[10], values[11]];
    assert!(
        (half - 0.5).abs() < 1e-6 && gone == 0.0 && (whole - 1.0).abs() < 1e-6,
        "the selected distance fade: half its span inside the reach {half}, at the reach \
         {gone}, a whole span inside {whole}"
    );
    assert!(
        (values[8] - (1.0 - half)).abs() < 1e-6,
        "a fully occluded sun ray darkens by the fade: {}",
        values[8]
    );
    assert_eq!(&values[12..16], &[1.0; 4], "an open floor sees the sun");
}

/// Run `body`, WGSL statements writing `results` vec4s into `result`, after the shipped helpers on
/// a surface-free compute device: `params` is the material's uniform and `texels` (`Rg8Unorm`,
/// `side` square, linearly filtered, repeating) the near store. Returns the vec4s' lanes in order.
pub(crate) fn evaluate(
    body: &str,
    params: &TerrainBlendParams,
    side: u32,
    texels: &[u8],
    results: u32,
) -> Vec<f32> {
    use bevy::tasks::block_on;
    use wgpu::util::DeviceExt;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("a headless compute adapter");
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        .expect("a headless compute device");
    let source = format!(
        r#"{}
@group(0) @binding(111) var<storage, read_write> result: array<vec4<f32>, {results}>;
@compute @workgroup_size(1)
fn check() {{
    {body}
}}
"#,
        helpers()
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("terrain blend helpers"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    // Declared rather than derived from `body`: a derived layout holds only the bindings `body`
    // reaches, and the bind group below binds all four.
    let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty,
        count: None,
    };
    let buffer = |ty: wgpu::BufferBindingType| wgpu::BindingType::Buffer {
        ty,
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            entry(100, buffer(wgpu::BufferBindingType::Uniform)),
            entry(
                102,
                wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            ),
            entry(
                109,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            ),
            entry(
                111,
                buffer(wgpu::BufferBindingType::Storage { read_only: false }),
            ),
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("check"),
        compilation_options: default(),
        cache: None,
    });
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(side * 2),
            rows_per_image: Some(side),
        },
        texture.size(),
    );
    let view = texture.create_view(&default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..default()
    });
    let mut uniform = bevy::render::render_resource::encase::UniformBuffer::new(Vec::new());
    uniform
        .write(params)
        .expect("the material's uniform layout");
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: uniform.as_ref(),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let size = u64::from(results) * 16;
    let result = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 100,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 102,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 109,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 111,
                resource: result.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&default());
    {
        let mut pass = encoder.begin_compute_pass(&default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&result, 0, &readback, 0, size);
    queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        send.send(result).expect("readback receiver");
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .expect("headless compute completes");
    receive.recv().unwrap().expect("readback maps");
    let bytes = readback.get_mapped_range(..);
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&b| f32::from_le_bytes(b))
        .collect()
}
