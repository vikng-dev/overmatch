//! Best practice: client-only, per-map sky admission, sun and analytic environment light.
//! `sun_light`, `sun_transform` and `level_ground_irradiance_lux` share one admitted sky;
//! `spawn_sun` is called only behind the world's window guard. The environment is filtered once
//! and shared by battlefield views. Each [`Bake`] holds the sky in world orientation
//! ([`world_direction`]), so its glow lies on the side the key light comes from. See
//! `.agents/docs/design/map-surroundings.md` for the design.

use std::{
    path::Path,
    sync::{Arc, LazyLock, Mutex},
};

use bevy::pbr::generate::{
    GeneratorBindGroups, GeneratorPipelines, RenderEnvironmentMap, filtering_system,
};
use bevy::prelude::*;
use bevy::render::render_resource::{CachedPipelineState, PipelineCache};
use bevy::render::sync_world::MainEntity;
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::shader::ShaderCacheError;
use serde::Deserialize;

use crate::provenance::{Sourced, from_shipped_str};
use crate::render_policy::CameraProfile;

static LIGHTING: LazyLock<Lighting> = LazyLock::new(|| {
    Lighting::load(&crate::map::map_dir(&crate::assets::asset_root()).join("view/sky.ron"))
});
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Lighting {
    /// Sun elevation above the horizon, degrees; low on purpose. At a grazing angle a normal-mapped
    /// bump swings from lit to self-shadowed across centimetres, so relief reads, where a high sun
    /// lights every ripple alike. Long shadows are also cover and a range cue.
    sun_elevation_deg: Sourced<f32>,
    /// Sun azimuth, degrees from +X toward +Z.
    sun_azimuth_deg: Sourced<f32>,
    /// Key-light intensity, lux. DERIVED: level-ground irradiance is illuminance times the sine
    /// of elevation, so these two tastes jointly determine how bright a flat surface reads.
    sun_illuminance_lux: Sourced<f32>,
    /// Sun colour — a restrained golden-hour warm. Subtle on purpose: this is a legibility change, and
    /// a strongly orange key would tint the albedo readings the gunner uses to tell surfaces apart.
    sun_srgb: Sourced<[f32; 3]>,
    /// Fill luminance, cd/m². MEASURED: this analytic sky's cosine-weighted upper hemisphere
    /// averages 0.41 of its scale. DERIVED example: a 600 cd/m² fill adds 246 cd/m² against the
    /// roughly 2300 cd/m² diffuse radiance of a 25000 lux key at 17 degrees, about 11 percent.
    /// The fill keeps shadowed faces legible without erasing the low sun's relief. It also gives
    /// metals something to reflect: a fully metallic surface has no diffuse response.
    sky_intensity_cd_m2: Sourced<f32>,
    sun_debug_distance_m: Sourced<f32>,
    zenith_linear: Sourced<[f32; 3]>,
    haze_linear: Sourced<[f32; 3]>,
    glow_linear: Sourced<[f32; 3]>,
    ground_linear: Sourced<[f32; 3]>,
    glow_exponent: Sourced<i32>,
    glow_strength: Sourced<f32>,
}
impl Lighting {
    fn load(path: &Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_else(|error| {
            panic!(
                "{}: sky: {error}; restore the map's required view/sky.ron",
                path.display()
            )
        });
        Self::parse(&path.display().to_string(), &text).unwrap_or_else(|error| {
            panic!("{error}; sky: author every required field with valid values and provenance")
        })
    }

    fn parse(file: &str, text: &str) -> Result<Self, String> {
        let data: Self = from_shipped_str(file, text)?;
        let refuse = |field: &str, fix: &str| format!("{file}: {field}: {fix}");
        if !data.sun_elevation_deg.0.is_finite()
            || !(0.0..90.0).contains(&data.sun_elevation_deg.0)
            || data.sun_elevation_deg.0 == 0.0
        {
            return Err(refuse(
                "sun_elevation_deg",
                "author a finite elevation strictly between the horizon and the zenith",
            ));
        }
        if !data.sun_azimuth_deg.0.is_finite() {
            return Err(refuse(
                "sun_azimuth_deg",
                "author a finite bearing in degrees",
            ));
        }
        for (field, value) in [
            ("sun_illuminance_lux", data.sun_illuminance_lux.0),
            ("sky_intensity_cd_m2", data.sky_intensity_cd_m2.0),
            ("sun_debug_distance_m", data.sun_debug_distance_m.0),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(refuse(field, "author a finite positive magnitude"));
            }
        }
        if data.glow_exponent.0 <= 0 {
            return Err(refuse("glow_exponent", "author a positive lobe exponent"));
        }
        if !data.glow_strength.0.is_finite() || !(0.0..=1.0).contains(&data.glow_strength.0) {
            return Err(refuse(
                "glow_strength",
                "author a finite blend strength in [0, 1]",
            ));
        }
        for (field, channels) in [
            ("sun_srgb", data.sun_srgb.0),
            ("zenith_linear", data.zenith_linear.0),
            ("haze_linear", data.haze_linear.0),
            ("glow_linear", data.glow_linear.0),
            ("ground_linear", data.ground_linear.0),
        ] {
            if channels
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            {
                return Err(refuse(field, "author finite channels in [0, 1]"));
            }
        }
        Ok(data)
    }
}
/// Resolution: shadow lookup offset suppresses numerical self-shadowing at grazing incidence.
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
        color: Color::srgb_from_array(LIGHTING.sun_srgb.0),
        illuminance: LIGHTING.sun_illuminance_lux.0,
        shadow_normal_bias: SUN_SHADOW_NORMAL_BIAS,
        ..default()
    }
}

/// The sun's irradiance on level ground, lux: `Lighting::sun_illuminance_lux` times the sine of the sun's
/// elevation ([`sun_elevation_deg`]). The key light a lit view-only mass shares with the ground it
/// rose from; the sky's share is not in it.
pub(crate) fn level_ground_irradiance_lux() -> f32 {
    LIGHTING.sun_illuminance_lux.0 * sun_elevation_deg().to_radians().sin()
}

/// The sun's elevation, degrees: `Lighting::sun_elevation_deg`, or a capture plan's
/// `scene.sun_elevation_deg` (`dev_capture::plan`, validated there). It moves the key light, its
/// sky and the baked terrain shadow (`terrain_shadow`) together, so a capture can judge terrain
/// shadows on a map whose slopes stay under the shipped sun.
fn sun_elevation_deg() -> f32 {
    crate::dev_capture::plan()
        .and_then(|plan| plan.scene.sun_elevation_deg)
        .unwrap_or(LIGHTING.sun_elevation_deg.0)
}

/// Unit vector pointing FROM the scene TOWARD the sun, from [`sun_elevation_deg`] and
/// `Lighting::sun_azimuth_deg`: the one direction the key light and the sky it is baked into share.
pub(crate) fn toward_sun() -> Vec3 {
    sun_direction(sun_elevation_deg(), LIGHTING.sun_azimuth_deg.0)
}

/// The one place that turns a sun's two angles, degrees, into a unit vector toward it: elevation
/// above the horizon, azimuth from +X toward +Z.
fn sun_direction(elevation_deg: f32, azimuth_deg: f32) -> Vec3 {
    let (sin_elevation, cos_elevation) = elevation_deg.to_radians().sin_cos();
    let (sin_azimuth, cos_azimuth) = azimuth_deg.to_radians().sin_cos();
    Vec3::new(
        cos_elevation * cos_azimuth,
        sin_elevation,
        cos_elevation * sin_azimuth,
    )
}

/// Where the sun sits, from [`toward_sun`]. Only the ROTATION reaches the shader — bevy fits the
/// shadow cascades around the camera, not around the light's position — so the authored stand-off is
/// purely so the entity reads sensibly in debug views.
pub(crate) fn sun_transform() -> Transform {
    Transform::from_translation(toward_sun() * LIGHTING.sun_debug_distance_m.0)
        .looking_at(Vec3::ZERO, Vec3::Y)
}

/// What a cubemap of [`sky_radiance`] is baked for. Every bake stores, at each texel, the radiance
/// of the world direction Bevy samples that texel for ([`world_direction`]), as `Rgba16Float`
/// LINEAR radiance ratios: the generator filters in `Rgba16Float` (its copy pass converts any
/// source), so an 8-bit source would only add banding.
#[derive(Clone, Copy)]
enum Bake {
    /// The source the environment light is filtered from. No sun disk: the `DirectionalLight`
    /// carries the sun's light, and a disk here would add it twice.
    Light,
}

impl Bake {
    /// Resolution: face size, pixels; a power of two, or `GeneratedEnvironmentMapLight` panics.
    /// Bevy convolves the light's source (Lambertian for diffuse, GGX per mip for specular) before
    /// anything samples it, so 128² resolves this smooth sky and keeps the startup filter cheap.
    fn face_px(self) -> u32 {
        match self {
            Bake::Light => 128,
        }
    }
}

/// The analytic golden-hour sky as LINEAR radiance ratios in `0..1` toward world direction `dir`,
/// under a sun toward `toward_sun`; the absolute level is `Lighting::sky_intensity_cd_m2`. Deep
/// blue overhead, warm haze at the horizon, a broad warm glow on the sun's side, and a dim warm
/// bounce below — the ground half matters as much as the sky half, because it is what keeps the
/// underside of a hull and the lower run of track darker than their tops (a uniform environment
/// would flatten them exactly the way ambient light does).
fn sky_radiance(look: &Lighting, toward_sun: Vec3, dir: Vec3, bake: Bake) -> Vec3 {
    let zenith = Vec3::from_array(look.zenith_linear.0);
    let haze = Vec3::from_array(look.haze_linear.0);
    let glow = Vec3::from_array(look.glow_linear.0);
    let ground = Vec3::from_array(look.ground_linear.0);

    let up = dir.y.clamp(-1.0, 1.0);
    if up >= 0.0 {
        // `sqrt` hugs the gradient to the horizon, where a real low-sun sky keeps its warmth.
        let t = up.sqrt();
        let base = haze.lerp(zenith, t);
        // The glow follows the KEY LIGHT's azimuth, so the reflection sliding along a track link
        // agrees with the shadow that link is casting.
        let sun_xz = toward_sun.xz().normalize_or_zero();
        let toward = dir.xz().normalize_or_zero().dot(sun_xz).max(0.0);
        base.lerp(
            glow,
            toward.powi(look.glow_exponent.0) * (1.0 - t) * look.glow_strength.0,
        )
    } else {
        match bake {
            Bake::Light => haze.lerp(ground, (-up).sqrt()),
        }
    }
}

/// The world direction Bevy samples cube direction `cube` for. Bevy treats cubemaps as
/// left-handed: the skybox (`skybox_fragment`) and the environment light
/// (`compute_cubemap_sample_dir`) both read a cubemap at (x, y, −z) of the world direction. The
/// flip is its own inverse.
fn world_direction(cube: Vec3) -> Vec3 {
    Vec3::new(cube.x, cube.y, -cube.z)
}

/// Direction through the centre of texel `(u, v)` of cubemap `face`, in cube coordinates and the
/// layer order wgpu (and glTF, and every cubemap format) uses: +X, −X, +Y, −Y, +Z, −Z.
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

/// Bake [`sky_radiance`] into six square layers, per [`Bake`].
fn bake_cubemap(look: &Lighting, toward_sun: Vec3, bake: Bake) -> Image {
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    let size = bake.face_px();
    let mut image = Image::new_fill(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        &[0; 8],
        TextureFormat::Rgba16Float,
        // Both worlds: the generator reads the asset back from `Assets<Image>` to size its
        // convolution, so the main-world copy has to survive extraction.
        RenderAssetUsages::all(),
    );
    let texel = |k: u32| 2.0 * (k as f32 + 0.5) / size as f32 - 1.0;
    for face in 0..6 {
        for y in 0..size {
            for x in 0..size {
                let dir = world_direction(cube_face_direction(face, texel(x), texel(y)));
                let [r, g, b] = sky_radiance(look, toward_sun, dir, bake).to_array();
                image
                    .set_color_at_3d(x, y, face, Color::linear_rgb(r, g, b))
                    .expect("a six-layer Rgba16Float image takes a colour at every texel");
            }
        }
    }
    image
}

/// The sky's environment light: the [`Bake::Light`] cubemap filtered into diffuse and specular maps ONCE per
/// app, then carried by every battlefield view as a plain `EnvironmentMapLight`. Inserted when the
/// first battlefield view appears, so an app with no camera (the dedicated server) never builds
/// the cubemap.
///
/// Once is enough because the sky never changes for the life of the app; a sky that changes must
/// start a new filtering. Bevy's `GeneratedEnvironmentMapLight` re-runs its compute passes on
/// every frame the component exists (vendored `bevy_pbr` `light_probe/generate.rs`,
/// `downsampling_system` and `filtering_system`), so the filter is despawned as soon as they have
/// been recorded.
///
/// The filtered maps exist only on the GPU: bevy creates both images with placeholder contents and
/// its passes write the textures. Nothing may mutate either `Image` asset: a mutation re-uploads
/// the placeholder, and the views lose the sky's light.
#[derive(Resource)]
enum SkyLight {
    /// The entity carrying `GeneratedEnvironmentMapLight`, until its passes are recorded.
    Filtering(Entity),
    /// The filtered maps, as every battlefield view carries them.
    Filtered(EnvironmentMapLight),
}

/// Main-world ids of the sky filters whose passes the render world has recorded. One `Arc` in both
/// worlds: the render world appends, the main world reads and clears.
#[derive(Resource, Clone, Default)]
struct RecordedSkyFilters(Arc<Mutex<Vec<Entity>>>);

/// The sky light's two halves: the main world's [`SkyLight`] systems, and the render world's
/// report of when the filter has run. An app without a render world (the dedicated server, the
/// headless fixtures) never sees its filter retire, and never needs to.
pub(crate) fn plugin(app: &mut App) {
    let recorded = RecordedSkyFilters::default();
    app.insert_resource(recorded.clone()).add_systems(
        Update,
        (
            retire_sky_filter.run_if(resource_exists::<SkyLight>),
            light_battlefield_views,
        )
            .chain(),
    );
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app.insert_resource(recorded).add_systems(
        Render,
        report_recorded_sky_filters
            .after(filtering_system)
            .before(RenderSystems::Render),
    );
}

/// Give every 3-D view of the battlefield image-based lighting from the shared sky, starting the
/// filter for the first one and attaching the filtered maps once [`SkyLight`] holds them. A view
/// whose profile draws no battlefield (the pen cam's inset and masks) lights its own stage.
///
/// Attached to VIEWS rather than spawned as a light: an environment map on a camera lights that
/// whole view, which is what "the sky" means here — as opposed to a `LightProbe`, which bounds it
/// to a region. Doing it in `sky` (rather than at each camera's spawn) is what keeps the game and
/// the armor sandbox honestly identical.
///
/// A frame-cost ablation (`SPIKE_ABLATE=env_light`, `render_cost`) leaves every view unlit by it.
fn light_battlefield_views(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    sky: Option<Res<SkyLight>>,
    ablate: Option<Res<crate::render_cost::Ablate>>,
    views: Query<(Entity, Option<&CameraProfile>), (With<Camera3d>, Without<EnvironmentMapLight>)>,
) {
    if ablate.is_some_and(|ablate| ablate.env_light) {
        return;
    }
    let mut views = views
        .iter()
        .filter(|(_, profile)| profile.is_none_or(|profile| profile.draws_battlefield()))
        .map(|(view, _)| view)
        .peekable();
    if views.peek().is_none() {
        return;
    }
    match sky.as_deref() {
        None => {
            let filter = commands
                .spawn((
                    Name::new("Sky light filter"),
                    GeneratedEnvironmentMapLight {
                        environment_map: images.add(bake_cubemap(
                            &LIGHTING,
                            toward_sun(),
                            Bake::Light,
                        )),
                        intensity: LIGHTING.sky_intensity_cd_m2.0,
                        ..default()
                    },
                ))
                .id();
            commands.insert_resource(SkyLight::Filtering(filter));
        }
        Some(SkyLight::Filtering(_)) => {}
        Some(SkyLight::Filtered(light)) => {
            for view in views {
                commands.entity(view).insert(light.clone());
            }
        }
    }
}

/// Keep the maps the filter wrote and despawn it, once the render world reports its passes
/// recorded. Bevy's `generate_environment_map_light` gives the filter its `EnvironmentMapLight`
/// before extraction can hand it to the render world, so a recorded filter always carries one.
fn retire_sky_filter(
    mut commands: Commands,
    mut sky: ResMut<SkyLight>,
    recorded: Res<RecordedSkyFilters>,
    maps: Query<&EnvironmentMapLight>,
) {
    let SkyLight::Filtering(filter) = *sky else {
        return;
    };
    let ran = {
        let mut recorded = recorded.0.lock().expect("sky filter report poisoned");
        let ran = recorded.contains(&filter);
        recorded.clear();
        ran
    };
    if !ran {
        return;
    }
    let light = maps
        .get(filter)
        .expect("a recorded sky filter carries the maps Bevy generated before extraction");
    *sky = SkyLight::Filtered(light.clone());
    commands.entity(filter).despawn();
    info!("sky light: filtered once; the filter is retired and the views share its maps");
}

/// Render-world half: report each sky filter whose passes bevy recorded this frame. Bevy's
/// `downsampling_system` and `filtering_system` record them for every entity carrying both
/// `GeneratorBindGroups` and `RenderEnvironmentMap`, once all five generator pipelines have
/// compiled. This runs after both and before `RenderSystems::Render`, whose
/// `process_pipeline_queue_system` is the next system to change pipeline states, so it sees the
/// readiness they saw.
///
/// A generator Bevy disabled (a GPU without compute shaders, a platform outside the targets) or a
/// generator shader that failed to compile would leave the views without the sky's light forever,
/// so both panic, naming the cause.
fn report_recorded_sky_filters(
    filters: Query<&MainEntity, (With<GeneratorBindGroups>, With<RenderEnvironmentMap>)>,
    pipelines: Option<Res<GeneratorPipelines>>,
    cache: Res<PipelineCache>,
    recorded: Res<RecordedSkyFilters>,
) {
    let pipelines = pipelines.expect(
        "sky light: Bevy disabled its environment-map generator, so this GPU lacks the compute \
         shaders the battlefield's sky light is filtered with",
    );
    for (name, id) in [
        ("copy", pipelines.copy),
        ("downsample_first", pipelines.downsample_first),
        ("downsample_second", pipelines.downsample_second),
        ("radiance", pipelines.radiance),
        ("irradiance", pipelines.irradiance),
    ] {
        match cache.get_compute_pipeline_state(id) {
            CachedPipelineState::Ok(_) => {}
            // Still compiling, or waiting on a shader or an import Bevy retries.
            CachedPipelineState::Queued
            | CachedPipelineState::Creating(_)
            | CachedPipelineState::Err(
                ShaderCacheError::ShaderNotLoaded(_)
                | ShaderCacheError::ShaderImportNotYetAvailable,
            ) => return,
            CachedPipelineState::Err(error) => panic!(
                "sky light: the environment-map generator's {name} pipeline failed ({error}); \
                 the battlefield views would never receive the sky's light"
            ),
        }
    }
    let mut recorded = recorded.0.lock().expect("sky filter report poisoned");
    for filter in &filters {
        debug!("sky light: filter passes recorded for {}", filter.id());
        recorded.push(filter.id());
    }
}

pub(crate) fn spawn_sun(commands: &mut Commands) {
    // Settings owns the cascade ladder; this spawn supplies only the key light and its pose.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    const FILE: &str = "fixture/view/sky.ron";
    const DATA: &str = include_str!("../assets/maps/kalinovo/view/sky.ron");

    /// Client startup requires readable, admitted sky data; every refusal names its file,
    /// field and repair so a map author can correct it before play.
    #[test]
    fn sky_file_failures_name_the_file_field_and_fix() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("target/sky-file-admission-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sky.ron");
        for (text, field, fix) in [
            (None, "sky:", "restore"),
            (Some("()"), "sun_elevation_deg", "author"),
            (Some("("), "sky:", "author"),
        ] {
            if let Some(text) = text {
                std::fs::write(&path, text).unwrap();
            }
            let error = std::panic::catch_unwind(|| Lighting::load(&path)).unwrap_err();
            let message = error
                .downcast_ref::<String>()
                .expect("the load gate formats its refusal");
            assert!(message.contains(&path.display().to_string()), "{message}");
            assert!(
                message.contains(field) && message.contains(fix),
                "{message}"
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The world refuses malformed authored light before startup, including test-only provenance.
    #[test]
    fn lighting_is_required_and_checked_before_play() {
        Lighting::parse(FILE, DATA).unwrap();
        let error = Lighting::parse(FILE, "()").unwrap_err();
        assert!(
            error.contains(FILE) && error.contains("sun_elevation_deg"),
            "{error}"
        );
        for (field, value) in [
            ("sun_elevation_deg", "90.0"),
            ("sun_azimuth_deg", "NaN"),
            ("sun_illuminance_lux", "0.0"),
            ("sun_debug_distance_m", "0.0"),
            ("sky_intensity_cd_m2", "0.0"),
            ("glow_exponent", "0"),
            ("glow_strength", "2.0"),
            ("sun_srgb", "(2.0, 0.0, 0.0)"),
        ] {
            let text = DATA
                .lines()
                .map(|line| {
                    if line.trim_start().starts_with(&format!("{field}:")) {
                        format!("    {field}: ({value}, Taste(\"lighting: admission fixture\")),")
                    } else {
                        line.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let error = Lighting::parse(FILE, &text).unwrap_err();
            assert!(error.contains(FILE) && error.contains(field), "{error}");
        }
        let error = Lighting::parse(FILE, &DATA.replace("Taste(", "Fixture(")).unwrap_err();
        assert!(
            error.contains(FILE) && error.contains("Fixture provenance is test-only"),
            "{error}"
        );
    }
    /// The constraints `GeneratedEnvironmentMapLight` PANICS on — square, power-of-two, and six
    /// faces — so a bad edit fails here rather than at first render.
    #[test]
    fn the_source_cubemap_matches_what_the_filter_requires() {
        let face = Bake::Light.face_px();
        assert!(face.is_power_of_two());
        let size = bake_cubemap(&LIGHTING, toward_sun(), Bake::Light)
            .texture_descriptor
            .size;
        assert_eq!(
            (size.width, size.height, size.depth_or_array_layers),
            (face, face, 6),
            "six square faces",
        );
    }

    /// Where the light bake's glow lies, which every lit surface relies on: on the side the
    /// directional light comes from. For two sun bearings asymmetric about both world axes, the
    /// bake, read the way Bevy samples a cubemap, is brightest along the horizon at the sun's own
    /// azimuth. A bake evaluated in cube coordinates mirrors the glow across the XY plane; a
    /// face-size test cannot see that. FIXTURE DATA: the bearings.
    #[test]
    fn the_light_bake_glows_on_the_suns_side() {
        let look = Lighting::parse(FILE, DATA).unwrap();
        for bearing_deg in [30.0_f32, 235.0] {
            let toward = sun_direction(look.sun_elevation_deg.0, bearing_deg);
            let bake = bake_cubemap(&look, toward, Bake::Light);
            let brightest = (0..360)
                .map(|degree| degree as f32)
                .max_by(|a, b| horizon(&bake, *a).total_cmp(&horizon(&bake, *b)))
                .unwrap();
            let sun = toward.z.atan2(toward.x).to_degrees();
            let off = (brightest - sun).rem_euclid(360.0);
            assert!(
                off.min(360.0 - off) <= 5.0,
                "sun at {sun}°, the glow brightest at {brightest}°"
            );
        }
    }

    /// The bake's summed linear RGB just above the horizon at world azimuth `azimuth_deg` (from +X
    /// toward +Z), read as Bevy reads a cubemap: at (x, y, −z) of the world direction
    /// (`compute_cubemap_sample_dir`), on the face and texel `dir_to_cube_uv` picks. Near the
    /// horizon, only the four side faces can dominate.
    fn horizon(bake: &Image, azimuth_deg: f32) -> f32 {
        let (sin, cos) = azimuth_deg.to_radians().sin_cos();
        let cube = Vec3::new(cos, 0.03, -sin);
        let (x, z) = (cube.x.abs(), cube.z.abs());
        let (face, u, v) = match (x >= z, cube.x > 0.0, cube.z > 0.0) {
            (true, true, _) => (0, -cube.z / x, -cube.y / x),
            (true, false, _) => (1, cube.z / x, -cube.y / x),
            (false, _, true) => (4, cube.x / z, -cube.y / z),
            (false, _, false) => (5, -cube.x / z, -cube.y / z),
        };
        let size = bake.width() as f32;
        let texel = |c: f32| (((c * 0.5 + 0.5) * size) as u32).min(bake.width() - 1);
        let colour = bake
            .get_color_at_3d(texel(u), texel(v), face)
            .expect("the bake has six layers")
            .to_linear();
        colour.red + colour.green + colour.blue
    }

    /// The sky light's steady state, which the frame cost depends on: once the render world
    /// reports the filter's passes recorded, no `GeneratedEnvironmentMapLight` is left for bevy to
    /// re-filter every frame, and every battlefield view (present then, or spawned later) carries
    /// the one filtered pair. Until that report the filter must survive: retiring it early leaves
    /// the views with maps nothing ever wrote.
    #[test]
    fn the_sky_is_filtered_once_and_then_shared_by_every_battlefield_view() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>().add_plugins(plugin);
        let view = app.world_mut().spawn(Camera3d::default()).id();
        app.update();
        let SkyLight::Filtering(filter) = *app.world().resource::<SkyLight>() else {
            panic!("the first battlefield view starts the filter");
        };

        // What bevy's generator gives the filter before the render world first sees it.
        let maps = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            EnvironmentMapLight {
                diffuse_map: images.add(Image::default()),
                specular_map: images.add(Image::default()),
                intensity: LIGHTING.sky_intensity_cd_m2.0,
                ..default()
            }
        };
        app.world_mut().entity_mut(filter).insert(maps.clone());
        app.update();
        assert!(
            app.world()
                .get::<GeneratedEnvironmentMapLight>(filter)
                .is_some(),
            "the filter is retired only once its passes have been recorded",
        );

        app.world()
            .resource::<RecordedSkyFilters>()
            .0
            .lock()
            .unwrap()
            .push(filter);
        app.update();
        let later_view = app.world_mut().spawn(Camera3d::default()).id();
        app.update();

        let world = app.world_mut();
        assert_eq!(
            world
                .query::<&GeneratedEnvironmentMapLight>()
                .iter(world)
                .count(),
            0,
            "nothing is left for bevy to filter again",
        );
        for lit in [view, later_view] {
            let light = world
                .get::<EnvironmentMapLight>(lit)
                .expect("every battlefield view carries the filtered maps");
            assert_eq!(
                (&light.diffuse_map, &light.specular_map),
                (&maps.diffuse_map, &maps.specular_map),
            );
            assert_eq!(light.intensity, LIGHTING.sky_intensity_cd_m2.0);
        }
    }
}
