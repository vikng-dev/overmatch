//! The battlefield: environment lighting and a locomotion test course now, real terrain later.
//! Also home to the ground-plane query that aiming and the camera both use — the seam to swap
//! for an Avian raycast once terrain has colliders.

use avian3d::prelude::{
    Collider, CollisionLayers, LayerMask, RigidBody, SpatialQuery, SpatialQueryFilter,
};
use bevy::prelude::*;

use crate::Layer;

/// The world's static terrain as DATA (track architecture §5): every terrain block in authoring
/// form — a unit cube posed/scaled by its Transform (the Avian collider idiom). Colliders are
/// spawned FROM this list, and the track module's analytic `BlockField` is built from the SAME
/// list, so the two representations cannot drift. `revision` bumps whenever the set changes
/// (map load; future streaming/destruction) so consumers know to rebuild and reseed.
#[derive(Resource)]
pub struct TerrainMap {
    pub revision: u64,
    pub blocks: Vec<Transform>,
}

/// Side length of the (square) ground plane, in metres — the fallback world's extent
/// (`terrain_grid::FIXTURE_EXTENT`), which is what the spawn map's UV mapping and the authority's
/// spawn clamp resolve against when no heightmap decoded.
const GROUND_SIZE: f32 = crate::terrain_grid::FIXTURE_EXTENT.world_size_m;
/// Thickness of the ground slab. Only the top face (at y=0) matters; the rest is buried.
const GROUND_THICKNESS: f32 = 1.0;

/// Sun elevation above the horizon, degrees — LOW, and deliberately so. Relief on a normal-mapped
/// surface is carried by the cosine between the light and the perturbed normal, so a high sun
/// (the old `(4, 8, 4)` placement was ~55°) flattens ground detail into near-uniform brightness
/// no matter how good the map is: every ripple faces the light about equally. At a grazing angle
/// the same bump swings from lit to self-shadowed across a few centimetres, which is what makes
/// the terrain read as SURFACE instead of a photograph. Also the honest tactical light for a tank
/// sim — long shadows are cover and a range cue.
const SUN_ELEVATION_DEG: f32 = 17.0;

/// Sun azimuth, degrees, measured from +X toward +Z. Unchanged from the old `(4, _, 4)` placement
/// (both horizontal components equal ⇒ 45°), so lowering the sun does not rotate which side of
/// the terrain's character is lit — only how hard.
const SUN_AZIMUTH_DEG: f32 = 45.0;

/// Sun intensity, lux. Raised with the elevation drop, not independently of it: a surface's lit
/// brightness carries a `sin(elevation)` factor, so the old 10 000 lux at ~55° put ~8 200 lux on
/// flat ground, and holding that at 17° would take ~28 000. 25 000 lands flat ground at ~7 300 —
/// about a sixth of a stop darker than before, so the scene reads as evening rather than murk,
/// without inventing brightness the geometry did not lose.
const SUN_ILLUMINANCE_LUX: f32 = 25_000.0;

/// Sun colour — a restrained golden-hour warm. Subtle on purpose: this is a legibility change, and
/// a strongly orange key would tint the albedo readings the gunner uses to tell surfaces apart.
const SUN_COLOR: Color = Color::srgb(1.0, 0.94, 0.86);

/// Shadow normal bias for the sun. Bevy's default (1.8) is tuned for a mid-height light; at 17°
/// the depth slope across one shadow-map texel grows by `cot(17°)/cot(55°)` ≈ 4.7×, which is
/// exactly the geometry that produces acne (a surface shadowing itself in stripes). This is a
/// deliberate but conservative nudge — the bias offsets the lookup ALONG the surface normal, so
/// overshooting detaches contact shadows ("peter-panning", a tank hovering over its own shadow).
/// If shadows look detached at the tracks, lower this before touching `shadow_depth_bias`.
const SUN_SHADOW_NORMAL_BIAS: f32 = 2.6;

/// The scene's sun as ONE definition, so the armor sandbox's overlay-layer copy
/// (`sandbox::spawn_overlay_light`) cannot drift from the light the world actually uses. Shadow
/// casting is the caller's call — the overlay light must not cast.
pub(crate) fn sun_light() -> DirectionalLight {
    DirectionalLight {
        color: SUN_COLOR,
        illuminance: SUN_ILLUMINANCE_LUX,
        shadow_normal_bias: SUN_SHADOW_NORMAL_BIAS,
        ..default()
    }
}

/// Unit vector pointing FROM the scene TOWARD the sun, from [`SUN_ELEVATION_DEG`] /
/// [`SUN_AZIMUTH_DEG`]. The one place that turns the two angles into a direction, so the key light
/// and the sky it is embedded in can never disagree about where the sun is.
fn toward_sun() -> Vec3 {
    let (sin_elevation, cos_elevation) = SUN_ELEVATION_DEG.to_radians().sin_cos();
    let (sin_azimuth, cos_azimuth) = SUN_AZIMUTH_DEG.to_radians().sin_cos();
    Vec3::new(
        cos_elevation * cos_azimuth,
        sin_elevation,
        cos_elevation * sin_azimuth,
    )
}

/// Where the sun sits, from [`toward_sun`]. Only the ROTATION reaches the shader — bevy fits the
/// shadow cascades around the camera, not around the light's position — so the 100 m stand-off is
/// purely so the entity reads sensibly in debug views.
pub(crate) fn sun_transform() -> Transform {
    Transform::from_translation(toward_sun() * 100.0).looking_at(Vec3::ZERO, Vec3::Y)
}

/// Face resolution of the environment light's source cubemap, pixels. Small on purpose: the source
/// is a smooth analytic sky with no small features, and bevy convolves it (Lambertian for diffuse,
/// GGX per mip for specular) before anything samples it, so 128² per face resolves everything this
/// sky contains — 393 kB, built in memory, never an asset on disk. MUST be a power of two;
/// `GeneratedEnvironmentMapLight` panics otherwise.
const SKY_CUBEMAP_FACE_PX: u32 = 128;

/// Environment-light luminance, cd/m² — the fill level, and the number to turn if this is wrong.
///
/// Balanced AGAINST the sun rather than picked for looks. The 17° key puts
/// `SUN_ILLUMINANCE_LUX · sin 17° ≈ 7300` lux on flat ground, i.e. ≈ 2300 cd/m² of diffuse
/// radiance. This sky's upper hemisphere averages 0.41 of its own scale seen from a flat surface
/// (cosine-weighted — measured, and pinned by `the_environment_fill_stays_a_fraction_of_the_sun`),
/// so 600 cd/m² adds ≈ 245: about 11 % of the sun. That is the whole point of the number: enough
/// that a shadowed face is lit by something, far too little to lift shadows into the lit side and
/// undo the relief the low sun was chosen to create. Raising this is the fastest way to flatten the
/// terrain again.
///
/// Metals are the reason it exists at all. A `metallic = 1.0` surface has NO diffuse response, so
/// with only a directional light it renders black except where it happens to mirror the sun — which
/// is exactly how the track links were reading. Specular reflection of this sky is what makes them
/// metal instead of shadow.
const SKY_ENVIRONMENT_INTENSITY: f32 = 600.0;

/// The analytic golden-hour sky as LINEAR radiance ratios in `0..1` for one direction; the absolute
/// level is [`SKY_ENVIRONMENT_INTENSITY`]. Deep blue overhead, warm haze at the horizon, a broad
/// warm glow on the sun's side, and a dim warm bounce below — the ground half matters as much as
/// the sky half, because it is what keeps the underside of a hull and the lower run of track darker
/// than their tops (a uniform environment would flatten them exactly the way ambient light does).
fn sky_radiance(dir: Vec3) -> Vec3 {
    /// Straight up: the deep part of a low-sun sky.
    const ZENITH: Vec3 = Vec3::new(0.22, 0.34, 0.55);
    /// The horizon band, warm with the long air path.
    const HAZE: Vec3 = Vec3::new(0.85, 0.65, 0.42);
    /// The sun's own quarter of the sky.
    const GLOW: Vec3 = Vec3::new(1.00, 0.78, 0.50);
    /// Downward: dim, warm sand bounce, not black — the terrain is lit sand, not a void.
    const GROUND: Vec3 = Vec3::new(0.10, 0.085, 0.07);

    let up = dir.y.clamp(-1.0, 1.0);
    if up >= 0.0 {
        // `sqrt` hugs the gradient to the horizon, where a real low-sun sky keeps its warmth.
        let t = up.sqrt();
        let base = HAZE.lerp(ZENITH, t);
        // The glow follows the KEY LIGHT's azimuth, so the reflection sliding along a track link
        // agrees with the shadow that link is casting.
        let sun_xz = toward_sun().xz().normalize_or_zero();
        let toward = dir.xz().normalize_or_zero().dot(sun_xz).max(0.0);
        base.lerp(GLOW, toward.powi(6) * (1.0 - t) * 0.6)
    } else {
        HAZE.lerp(GROUND, (-up).sqrt())
    }
}

/// Direction through the centre of texel `(u, v)` of cubemap `face`, in the layer order wgpu (and
/// glTF, and every cubemap format) uses: +X, −X, +Y, −Y, +Z, −Z.
fn cube_face_direction(face: u32, u: f32, v: f32) -> Vec3 {
    match face {
        0 => Vec3::new(1.0, -v, -u),
        1 => Vec3::new(-1.0, -v, u),
        2 => Vec3::new(u, 1.0, v),
        3 => Vec3::new(u, -1.0, -v),
        4 => Vec3::new(u, -v, 1.0),
        _ => Vec3::new(-u, -v, -1.0),
    }
    .normalize()
}

/// Build the environment light's source cubemap from [`sky_radiance`]. Six square layers of
/// `Rgba8Unorm` — LINEAR, not sRGB, because what we author here are radiance ratios, not colours a
/// display should decode.
fn sky_cubemap() -> Image {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::Image;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    let size = SKY_CUBEMAP_FACE_PX;
    let mut data = Vec::with_capacity((size * size * 6 * 4) as usize);
    for face in 0..6 {
        for y in 0..size {
            for x in 0..size {
                let texel = |k: u32| 2.0 * (k as f32 + 0.5) / size as f32 - 1.0;
                let radiance = sky_radiance(cube_face_direction(face, texel(x), texel(y)));
                data.extend_from_slice(&[
                    (radiance.x * 255.0).round() as u8,
                    (radiance.y * 255.0).round() as u8,
                    (radiance.z * 255.0).round() as u8,
                    u8::MAX,
                ]);
            }
        }
    }
    Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        // Both worlds: the generator reads the asset back from `Assets<Image>` to size its
        // convolution, so the main-world copy has to survive extraction.
        RenderAssetUsages::all(),
    )
}

/// Give every 3-D view image-based lighting, once, from the shared sky.
///
/// Attached to VIEWS rather than spawned as a light: an environment map on a camera lights that
/// whole view, which is what "the sky" means here — as opposed to a `LightProbe`, which bounds it
/// to a region. Doing it in `world` (rather than at each camera's spawn) is what keeps the game and
/// the armor sandbox honestly identical, and it costs nothing where there is no camera: the
/// dedicated server never matches this query, so it never even builds the cubemap.
fn attach_environment_light(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut sky: Local<Option<Handle<Image>>>,
    views: Query<Entity, (With<Camera3d>, Without<GeneratedEnvironmentMapLight>)>,
) {
    if views.is_empty() {
        return;
    }
    let sky = sky.get_or_insert_with(|| images.add(sky_cubemap()));
    for view in &views {
        commands.entity(view).insert(GeneratedEnvironmentMapLight {
            environment_map: sky.clone(),
            intensity: SKY_ENVIRONMENT_INTENSITY,
            ..default()
        });
    }
}

pub fn plugin(app: &mut App) {
    // Decode the heightmap FIRST (synchronous, ADR-0014), then spawn whichever world it selects:
    // the heightfield when the grid decoded, the flat slab + authored course otherwise.
    app.add_systems(
        Startup,
        (crate::terrain_grid::decode_height_grid, spawn_environment).chain(),
    );
    app.add_systems(
        Update,
        (report_failed_terrain_map, attach_environment_light),
    );
    // The terrain LOD ladder's adaptive half: the levels are generated in `spawn_environment`
    // above, and this keeps their switch distances honest as the optic toggles and the window
    // resizes. Inert until a ladder exists, so the dedicated server pays nothing.
    app.add_plugins(crate::terrain_lod::plugin);
}

/// How a terrain surface map's bytes must be interpreted at LOAD time — the one texture decision
/// that cannot be corrected downstream (the sampler hands the shader whatever the GPU format says).
/// The diffuse carries COLOUR and is authored in sRGB; the normal and ARM maps carry DATA — a
/// tangent-space direction, and three material scalars — and an sRGB transfer applied to those
/// bends every direction and every roughness value.
///
/// Still ours to state even though the maps are KTX2: bevy does NOT read the container's transfer
/// function for a UASTC payload — `is_srgb` is what picks the sRGB variant of the transcode target
/// (`Astc { channel: UnormSrgb }` / `Bc7RgbaUnormSrgb` vs the plain `Unorm` forms).
#[derive(Clone, Copy)]
enum MapEncoding {
    Srgb,
    Linear,
}

/// Anisotropic-filter taps for the ground. The terrain is the one surface always seen at grazing
/// angles out to the horizon, which is exactly the case isotropic filtering blurs; 8 is the usual
/// quality/cost knee (16 costs more for little visible gain at this texel density). Only meaningful
/// because the maps carry mip chains — anisotropy is a rule for choosing among mip levels, so on
/// the old single-level PNGs it cost sampling work and bought nothing.
const TERRAIN_ANISOTROPY: u16 = 8;

/// Load one terrain surface map with the sampler EVERY terrain texture needs: repeat addressing
/// (bevy's default clamps, which would smear the first [`crate::terrain_grid::TEXTURE_TILE_M`]
/// tile across the whole map), trilinear filtering across the mip chain, and anisotropy. Async load
/// is fine — this is pure view (ADR-0014), nothing sim-side reads it — but a FAILED load is fatal
/// ([`report_failed_terrain_map`]). The sampler is orthogonal to the container: the same descriptor
/// rides the KTX2 maps exactly as it rode the PNGs.
fn terrain_map(
    asset_server: &AssetServer,
    path: &'static str,
    encoding: MapEncoding,
) -> Handle<Image> {
    use bevy::image::{
        ImageAddressMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor,
    };
    let is_srgb = matches!(encoding, MapEncoding::Srgb);
    asset_server
        .load_builder()
        .with_settings(move |settings: &mut ImageLoaderSettings| {
            settings.is_srgb = is_srgb;
            let mut sampler = ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                ..ImageSamplerDescriptor::default()
            };
            // Sets the three filters to Linear as well — wgpu REQUIRES that for anisotropy > 1.
            sampler.set_anisotropic_filter(TERRAIN_ANISOTROPY);
            settings.sampler = ImageSampler::Descriptor(sampler);
        })
        .load(path)
}

/// The terrain surface maps whose load must succeed, held by id for [`report_failed_terrain_map`].
/// Only inserted by the heightmap world with a window — the flat slab / authored course and the
/// dedicated server never load them, so they can never fail there.
#[derive(Resource)]
struct TerrainMaps([AssetId<Image>; 3]);

/// Surface a failed terrain-map load instead of swallowing it (ADR-0011, the same stance as
/// `spec::report_failed_spec`). A texture that fails to decode does NOT draw untextured: bevy
/// leaves the material unprepared, so the whole ground silently stops rendering — the exact
/// failure the `jpeg` feature note in `Cargo.toml` records us hitting once already. Required,
/// in-repo asset ⇒ panic in every build.
fn report_failed_terrain_map(
    mut failures: MessageReader<bevy::asset::AssetLoadFailedEvent<Image>>,
    required: Option<Res<TerrainMaps>>,
) {
    let Some(required) = required else {
        return;
    };
    for failure in failures.read() {
        if required.0.contains(&failure.id) {
            let (path, err) = (&failure.path, &failure.error);
            error!("required terrain map {path} failed to load: {err}");
            panic!("required terrain map {path} failed to load: {err}");
        }
    }
}

fn spawn_environment(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    grid: Option<Res<crate::terrain_grid::HeightGrid>>,
    // The one parse of the map's manifest (`terrain_grid::decode_height_grid` publishes it), which
    // the scatter's placement is read out of.
    manifest: Option<Res<crate::map::MapManifest>>,
    windows: Query<&Window>,
    scale: Option<Res<crate::render_scale::RenderScale>>,
    asset_server: Res<AssetServer>,
) {
    let mut blocks: Vec<Transform> = Vec::new();
    // The sun (see [`sun_light`]). NO cascade config is spawned here on purpose: `settings::
    // apply_settings` is the single writer of the cascade ladder, so that the settings page and the
    // picture cannot disagree. As of 2026-07-28 it writes 3 splits out to 350 m with the first bound
    // at 40 m (`settings::SHADOW_CASCADES`, `ShadowDistance`'s default rung and
    // `settings::SHADOW_FIRST_CASCADE_FAR_BOUND_M`) — none of which are bevy's defaults of 4 / 150 m
    // / 10 m any more, and all of which are live player-facing rows. What IS decided here is the
    // light itself: cascades are fitted around the CAMERA, so the low sun does not stretch them;
    // what a low sun does stretch is the shadow texel's footprint along the light, which is what
    // [`SUN_SHADOW_NORMAL_BIAS`] answers.
    commands.spawn((
        DirectionalLight {
            shadow_maps_enabled: true,
            ..sun_light()
        },
        sun_transform(),
        // WHAT THE SUN LIGHTS AND CASTS FROM. Not optional and not cosmetic: the vendored bevy_pbr
        // patch gives every shadow view its LIGHT's mask, so a sun with no profile is layer-0-only
        // and silently stops shadowing everything `render_policy` has moved off the world channel —
        // the local tank's whole body, and the track shadow ribbon. Pinned by
        // `render_policy::tests::the_sun_reaches_every_channel`.
        crate::render_policy::LightProfile::BattlefieldSun,
    ));

    // The heightmap world: when the grid decoded, the heightfield IS the world — no flat slab,
    // no authored test course. The oracle's ground term comes from the grid; `TerrainMap` carries
    // only what stands ON it (the scatter's buildings, below), on the same revision semantics
    // `TrackField` rebuilds from.
    if let Some(grid) = grid {
        // The view layer's parry cap must reach across the map the grid actually is (ADR-0011): a
        // world wider than [`VIEW_CAST_MAX_M`]'s diagonal clips aim and camera picks at the cap
        // instead of at the ground, and the clip is invisible — the miss fallback looks like sky.
        assert!(
            VIEW_CAST_MAX_M >= grid.world_size() * std::f32::consts::SQRT_2,
            "VIEW_CAST_MAX_M ({VIEW_CAST_MAX_M} m) must cover the {} m world's diagonal",
            grid.world_size(),
        );
        commands.spawn((
            Transform::IDENTITY,
            RigidBody::Static,
            crate::terrain_grid::heightfield_collider(&grid),
            CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
        ));
        // The render mesh is view-only: windowed compositions have a primary window; the
        // dedicated server (and the headless harness) has none and must not pay for it.
        if !windows.is_empty() {
            // The ground surface pack (Poly Haven, CC0 — see the pack folder's `cc.txt`),
            // world-space tiled by the mesh's UVs every `terrain_grid::TEXTURE_TILE_M`.
            let diffuse = terrain_map(
                &asset_server,
                crate::terrain_grid::TEXTURE_PATH,
                MapEncoding::Srgb,
            );
            let normal = terrain_map(
                &asset_server,
                crate::terrain_grid::NORMAL_PATH,
                MapEncoding::Linear,
            );
            let arm = terrain_map(
                &asset_server,
                crate::terrain_grid::ARM_PATH,
                MapEncoding::Linear,
            );
            commands.insert_resource(TerrainMaps([diffuse.id(), normal.id(), arm.id()]));
            let material = materials.add(StandardMaterial {
                base_color_texture: Some(diffuse),
                // `nor_gl` is the OpenGL convention (+Y green points UP in tangent space) — what
                // bevy and glTF expect, so NO flip. The pack's `nor_dx` sibling would need this
                // `true`; picking the wrong one inverts every bump into a dent under a low sun,
                // which is why the file name carries the convention.
                normal_map_texture: Some(normal),
                // glTF ORM packing — R = ambient occlusion, G = roughness, B = metallic. Poly
                // Haven's `arm` IS that layout, so ONE image feeds both slots bevy reads it
                // through (occlusion takes R, metallic-roughness takes G/B).
                occlusion_texture: Some(arm.clone()),
                metallic_roughness_texture: Some(arm),
                // The shader MULTIPLIES these factors into the map (`perceptual_roughness *=
                // mr.g`, `metallic *= mr.b` — bevy_pbr's `pbr_fragment.wgsl`), so 1.0 passes the
                // roughness map through unscaled (the old 0.95 would have darkened every value).
                // Metallic stays 0.0 rather than 1.0: the pack's B channel is all-zero (checked
                // on import), so the render is identical either way, and hard-zero keeps a
                // dielectric ground from turning to metal on a pack swap.
                perceptual_roughness: 1.0,
                metallic: 0.0,
                ..default()
            });
            // The ONE-SURFACE invariant (terrain_grid module doc): the drawn ground is built
            // from the grid's own samples with the collider's own cell diagonal — identical
            // geometry, chunked into world-space tiles (positions are absolute, transforms
            // identity) purely so bevy frustum-culls per tile.
            //
            // Each tile ships as a LADDER rather than a single mesh (`terrain_lod`): the exact
            // surface up close, then RTIN levels whose declared deviation is sub-pixel at the
            // distance bevy switches them in. Generated HERE, from the in-memory grid, so there is
            // no build product that can go stale against the surface the sim reads. Tangent
            // generation (ADR-0011, required with no fallback) happens per level inside `spawn`.
            crate::terrain_lod::spawn(
                &mut commands,
                &mut meshes,
                &material,
                &grid,
                terrain_lod_view(windows.iter().next(), scale.as_deref()),
            );
        }
        // The map's object scatter (`scatter`): graybox proxies posed from the shipped level file
        // onto THIS grid, colliders on every composition and meshes only where there is a window.
        // Appended to `blocks` before the resource is inserted, so a building enters the analytic
        // track field through the same list the authored course's cuboids do. The showcase world is
        // excluded for the reason its grid is flat: scenery in front of the thing being looked at.
        if let Some(manifest) = manifest.filter(|_| !crate::lod_showcase::enabled()) {
            crate::scatter::spawn(
                &mut commands,
                &mut meshes,
                &mut materials,
                &manifest,
                &grid,
                &mut blocks,
                !windows.is_empty(),
            );
        }
        commands.insert_resource(TerrainMap {
            revision: 0,
            blocks,
        });
        return;
    }

    // The ground: a static slab whose top face sits at y=0 — the same plane the analytic
    // `ground_distance` assumes, so aim/camera are unaffected.
    spawn_block(
        &mut commands,
        &mut blocks,
        meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        materials.add(Color::srgb(0.32, 0.42, 0.28)),
        Transform::from_xyz(0.0, -GROUND_THICKNESS / 2.0, 0.0).with_scale(Vec3::new(
            GROUND_SIZE,
            GROUND_THICKNESS,
            GROUND_SIZE,
        )),
    );

    // The locomotion test course — deliberate, known geometry (not a scenic map) laid out down
    // the −Z lane in front of spawn, so each obstacle isolates one track behaviour and you
    // can tell the *sim* from the *terrain*. All on the Terrain layer, so the wheel rays read it
    // identically to the ground.
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let ramp_mat = materials.add(Color::srgb(0.45, 0.38, 0.28));
    let bump_mat = materials.add(Color::srgb(0.40, 0.33, 0.24));
    spawn_test_course(&mut commands, &mut blocks, &cube, &ramp_mat, &bump_mat);
    commands.insert_resource(TerrainMap {
        revision: 0,
        blocks,
    });
}

/// The view profile the terrain ladder is FIRST wired for, at Startup — before any camera has
/// spawned and therefore before any fov is knowable.
///
/// Deliberately the NARROWEST view the game has (the gunner optic): a narrow field demands the
/// finest geometry, so seeding with it means the first frames are over-detailed rather than
/// under-detailed. `terrain_lod::adapt_ranges` replaces it with the live view on the first frame
/// that has a window and a camera, at human rate thereafter. A window bevy has not sized yet
/// reports zero height, which `ViewFacts::new` reads as ABSENT rather than as a one-pixel viewport.
///
/// The FIELD is seeded, not read: no camera exists yet. The rendered HEIGHT comes out of
/// `crate::view`'s own expression of it, so the seed and the live view cannot disagree about what
/// the render scale does.
fn terrain_lod_view(
    window: Option<&Window>,
    scale: Option<&crate::render_scale::RenderScale>,
) -> crate::view::ViewProfile {
    crate::terrain_lod::terrain_view(crate::view::ViewFacts::new(
        crate::camera::GUNNER_FOV_FALLBACK,
        crate::view::ViewFacts::rendered_height_px(window, scale),
    ))
}

/// Spawn a static, unit-cube collision block scaled/posed by `transform` (the Avian idiom: a
/// `Collider::cuboid(1,1,1)` that the Transform's scale stretches), on the Terrain layer — and
/// record it in the [`TerrainMap`] block list (the single terrain data source).
fn spawn_block(
    commands: &mut Commands,
    blocks: &mut Vec<Transform>,
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    transform: Transform,
) {
    blocks.push(transform);
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material),
        transform,
        RigidBody::Static,
        Collider::cuboid(1.0, 1.0, 1.0),
        CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
    ));
}

/// The four-obstacle locomotion course. Each obstacle is a static cuboid (or row of them) sized
/// to isolate one thing the belt contact model does. Reuses one unit-cube mesh and two
/// materials, cloned per block.
fn spawn_test_course(
    commands: &mut Commands,
    blocks: &mut Vec<Transform>,
    cube: &Handle<Mesh>,
    ramp_mat: &Handle<StandardMaterial>,
    bump_mat: &Handle<StandardMaterial>,
) {
    // 1. Graduated climbs — ramps at 10°/20°/30°, side by side, to compare pitch and find the
    //    climb limit. (With ~200 kN total thrust vs ~456 kN weight, 20° climbs but 30° stalls —
    //    gravity-along-slope exceeds thrust — so this also shows where it gives out.) Each is a
    //    slab tilted about X and sunk so its low edge's top sits ~1 m under the ground slab: the
    //    upslope crosses y=0 flush (step-free entry), the high edge a crest with a drop beyond.
    //    Low-edge top y = center_y + (thickness/2)·cosθ − (run/2)·sinθ; solve for center_y at −1 m.
    let (run, width, thick) = (10.0_f32, 10.0_f32, 2.0_f32);
    for (i, deg) in [10.0_f32, 20.0, 30.0].into_iter().enumerate() {
        let (sin, cos) = deg.to_radians().sin_cos();
        let center_y = -1.0 - (thick / 2.0) * cos + (run / 2.0) * sin;
        let x = (i as f32 - 1.0) * 14.0; // −14, 0, +14
        spawn_block(
            commands,
            blocks,
            cube.clone(),
            ramp_mat.clone(),
            Transform::from_xyz(x, center_y, -40.0)
                .with_rotation(Quat::from_rotation_x(deg.to_radians()))
                .with_scale(Vec3::new(width, thick, run)),
        );
    }

    // 2. Side-slope — a banked lane tilted about Z, driven ALONG Z so the tank is canted sideways:
    //    shows roll, lateral weight transfer, and whether it holds the face or slides off. Centred
    //    at y=0 so the banked top crosses ground near the lane centre (a roughly flush approach).
    spawn_block(
        commands,
        blocks,
        cube.clone(),
        ramp_mat.clone(),
        Transform::from_xyz(38.0, 0.0, -45.0)
            .with_rotation(Quat::from_rotation_z(18.0_f32.to_radians()))
            .with_scale(Vec3::new(16.0, 2.0, 26.0)),
    );

    // 3. Step / curb — a low box driven over: front wheels lift over the hard edge, then the rear.
    //    Single-wheel articulation against a vertical edge (top at y=0.4).
    spawn_block(
        commands,
        blocks,
        cube.clone(),
        bump_mat.clone(),
        Transform::from_xyz(0.0, 0.2, -70.0).with_scale(Vec3::new(14.0, 0.4, 4.0)),
    );

    // 4. Washboard — a row of low bumps; wheels rise and fall independently while the hull stays
    //    composed (the most legible "terrain following works" demo). Boxes approximate rounded bumps
    //    — a round profile is a later refinement.
    for i in 0..6 {
        let z = -82.0 - i as f32 * 1.6;
        spawn_block(
            commands,
            blocks,
            cube.clone(),
            bump_mat.clone(),
            Transform::from_xyz(0.0, 0.12, z).with_scale(Vec3::new(14.0, 0.25, 0.6)),
        );
    }
}

/// Longest CAST any view-layer ground/aim ray needs, metres: no terrain sightline can exceed the
/// world's full diagonal — `world_size·√2`, 2 121.3 m on the shipped 1 500 m map — from any
/// in-world origin, and this adds headroom on top. The world's side is the MAP's to declare
/// (`map::MapManifest`), so the bound is checked against the decoded grid in
/// [`spawn_environment`] rather than at compile time.
/// Purely a parry-traversal cap for the view layer (aim/sight picks, the bore dot, the camera
/// pull-in): `aim::MAX_RANGE` (10 km) keeps its separate role as the far "sky" FALLBACK distance,
/// so committed aim points and all in-range behavior are unchanged — nothing exists between the
/// diagonal and 10 km for a ray to hit. Sim code must not read this.
pub(crate) const VIEW_CAST_MAX_M: f32 = 2_500.0;

/// Distance along `ray` to the terrain, capped at `max`, falling back to `max` when the ray
/// misses (sky / above the horizon). A world raycast against the `Terrain` layer ONLY — the orbit
/// camera's ground pull-in, which must ignore tanks (a tank crossing behind the player must not
/// yank the camera in). Aim rays use `aim::aim_distance` instead, which adds the `Armor` layer so
/// the aim dots predict what a shell would actually meet, tanks included. The CAST is clamped to
/// [`VIEW_CAST_MAX_M`] (nothing exists beyond the world diagonal); the miss fallback stays `max`.
pub fn ground_distance(spatial: &SpatialQuery, ray: Ray3d, max: f32) -> f32 {
    spatial
        .cast_ray(
            ray.origin,
            ray.direction,
            max.min(VIEW_CAST_MAX_M),
            true,
            &SpatialQueryFilter::from_mask(Layer::Terrain),
        )
        .map(|hit| hit.distance)
        .unwrap_or(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative luminance — the eye's weighting, so "brighter" in these tests means what it means
    /// on screen rather than what the raw channel sum says.
    fn luminance(c: Vec3) -> f32 {
        0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
    }

    /// Deterministic cosine-weighted average of the sky over the upper hemisphere: what a flat,
    /// upward-facing diffuse surface actually integrates (a nested elevation/azimuth lattice, so
    /// the number is reproducible rather than sampled).
    fn cosine_weighted_sky_average() -> f32 {
        let (mut total, mut weight) = (0.0f32, 0.0f32);
        for i in 0..180 {
            let elevation = (i as f32 + 0.5) / 180.0 * std::f32::consts::FRAC_PI_2;
            let (sin_e, cos_e) = elevation.sin_cos();
            for j in 0..360 {
                let azimuth = (j as f32 + 0.5) / 360.0 * std::f32::consts::TAU;
                let (sin_a, cos_a) = azimuth.sin_cos();
                let dir = Vec3::new(cos_e * cos_a, sin_e, cos_e * sin_a);
                // Solid angle ∝ cos(elevation), Lambert's law contributes another sin(elevation).
                let w = cos_e * sin_e;
                total += luminance(sky_radiance(dir)) * w;
                weight += w;
            }
        }
        total / weight
    }

    /// THE balance the sun slice depends on, in a test instead of a comment: the environment map
    /// must stay a FILL light. If a future tweak pushes the sky's contribution toward the sun's,
    /// shadows lift, and the whole reason for a 17° key — relief you can read on the ground — is
    /// gone. This fails long before that is visible in a screenshot.
    #[test]
    fn the_environment_fill_stays_a_fraction_of_the_sun() {
        let average = cosine_weighted_sky_average();
        assert!(
            (0.35..0.50).contains(&average),
            "the sky's cosine-weighted average is {average:.3}; SKY_ENVIRONMENT_INTENSITY's \
             documented arithmetic assumes ≈ 0.41",
        );
        // Diffuse radiance the sun puts on flat ground, and what the sky adds on top of it.
        let sun = SUN_ILLUMINANCE_LUX * SUN_ELEVATION_DEG.to_radians().sin() / std::f32::consts::PI;
        let fill = SKY_ENVIRONMENT_INTENSITY * average;
        let ratio = fill / sun;
        assert!(
            (0.05..0.20).contains(&ratio),
            "environment fill is {:.0} cd/m² against the sun's {sun:.0} ({:.0} %) — outside the \
             5–20 % band that keeps the scene sun-dominated",
            fill,
            ratio * 100.0,
        );
    }

    /// The sky must be DIRECTIONAL, which is the whole difference between it and `AmbientLight`.
    /// A uniform environment lights a hull's underside exactly like its deck and reads as flat
    /// fill; this one has to stay much brighter above than below.
    #[test]
    fn the_sky_is_brighter_above_than_below() {
        let zenith = luminance(sky_radiance(Vec3::Y));
        let nadir = luminance(sky_radiance(Vec3::NEG_Y));
        assert!(
            zenith > nadir * 3.0,
            "zenith {zenith:.3} vs ground bounce {nadir:.3} — too uniform to shape anything",
        );
    }

    /// The warm quarter of the sky sits on the KEY LIGHT's side, so a reflection travelling along a
    /// track link agrees with the shadow that link casts.
    #[test]
    fn the_warm_glow_follows_the_sun_azimuth() {
        let horizon = |dir: Vec3| sky_radiance((dir * 0.99 + Vec3::Y * 0.05).normalize());
        let sun_side = horizon(toward_sun().with_y(0.0).normalize());
        let away = horizon(-toward_sun().with_y(0.0).normalize());
        assert!(
            luminance(sun_side) > luminance(away),
            "the sun's side of the horizon must be the bright one",
        );
        assert!(
            sun_side.x - sun_side.z > away.x - away.z,
            "and the warm one (more red over blue)",
        );
    }

    /// The constraints `GeneratedEnvironmentMapLight` PANICS on — square, power-of-two, and six
    /// faces — plus the byte count, so a bad edit fails here rather than at first render.
    #[test]
    fn the_source_cubemap_matches_what_the_filter_requires() {
        let sky = sky_cubemap();
        let size = sky.texture_descriptor.size;
        assert!(SKY_CUBEMAP_FACE_PX.is_power_of_two());
        assert_eq!(
            (size.width, size.height),
            (SKY_CUBEMAP_FACE_PX, SKY_CUBEMAP_FACE_PX)
        );
        assert_eq!(size.depth_or_array_layers, 6, "a cubemap is six faces");
        assert_eq!(
            sky.data.as_ref().map(Vec::len),
            Some((SKY_CUBEMAP_FACE_PX * SKY_CUBEMAP_FACE_PX * 6 * 4) as usize),
            "four bytes per texel across six faces",
        );
    }
}
