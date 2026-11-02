//! The game's damage-model view: impact dressing for [`Impact`], game state only.
//!
//! Invariant (ADR-0014): this module only reads simulation events and spawns transient render
//! entities. Resolved substance responses choose powder and sparks; no visual entity affects game state.
//! Required parameters live in `assets/vfx/presentation.ron`; stand-ins name their missing
//! emission laws. The authored world-space scale is independent of viewer distance.

use super::look::impact as look;
use crate::substances::SurfaceFeedback;

use bevy::camera::Exposure;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::BTreeMap;

use crate::ballistics::{Impact, ImpactOutcome, ImpactSurface};
use crate::rounds::ImpactStyle;
use crate::view::PlayerView;

use super::ViewRng;
use super::billboard::{
    BILLBOARD_CAP, BillboardLook, BillboardMaterials, BillboardRing, BillboardSpec, Flipbook,
    VfxParams, gradient_lut, soft_particle_depth, spawn_ballistic_billboard, spawn_billboard,
    unit_quad,
};

// Alpha blending lets dust darken and occlude rather than behave as an emissive effect.

/// Resolution: scar population cap bounds persistent decal slots.
pub(super) const GROUND_MARK_CAP: usize = 16;

/// The ground scars' own ring, so a sub-second billboard storm cannot evict a multi-second scar.
/// Its slots lie past the shared ring's, inside the lane table.
#[derive(Resource)]
pub(super) struct GroundMarkRing(pub(super) BillboardRing);

impl Default for GroundMarkRing {
    fn default() -> Self {
        Self(BillboardRing::new(BILLBOARD_CAP, GROUND_MARK_CAP))
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<GroundMarkRing>()
        .add_systems(Startup, setup_impact_assets)
        // The ship impact read: a view-side subscriber to `ballistics`' sim `Impact` event
        // (ADR-0014) — the same seam the debug marker and the sandbox subscribe to. Dust/ping/sparks
        // ride the shared billboard ring + ager, so no impact-local ring or aging system is needed.
        .add_observer(spawn_impact_read);
}

/// Shared geometry, atlases and material palettes; the prewarm rig uses the spark material.
#[derive(Resource)]
pub(super) struct ImpactAssets {
    pub(super) quad: Handle<Mesh>,
    dust_atlas: Handle<Image>,
    ping_atlas: Handle<Image>,
    spark_atlas: Handle<Image>,
    spark_lut: Handle<Image>,
    /// One powder palette per authored hard-substance colour.
    powder_luts: BTreeMap<[u8; 3], Handle<Image>>,
    /// Hot contact and spark palette; compact sparks use the warmer `spark_lut`.
    hot_lut: Handle<Image>,
    ground_palettes: super::muzzle::DustPalettes,
}

#[cfg(test)]
impl ImpactAssets {
    /// Every handle default: the tests spawn real billboards and make real family materials, but no
    /// render app samples the textures behind the handles. Shared with [`super::scatter_hit`]'s
    /// harness, which drives the same read from a crossing instead of from an [`Impact`].
    pub(super) fn test_stub() -> Self {
        Self {
            quad: Handle::default(),
            dust_atlas: Handle::default(),
            ping_atlas: Handle::default(),
            spark_atlas: Handle::default(),
            spark_lut: Handle::default(),
            powder_luts: [(SurfaceFeedback::fixture().powder, Handle::default())]
                .into_iter()
                .collect(),
            hot_lut: Handle::default(),
            ground_palettes: super::muzzle::DustPalettes::default(),
        }
    }
}

impl ImpactAssets {
    fn powder_lut(&self, feedback: SurfaceFeedback) -> Handle<Image> {
        self.powder_luts
            .get(&feedback.powder)
            .expect("all hard-substance powder palettes are preloaded")
            .clone()
    }

    /// Alpha-blended powder through its resolved palette; atlas frame lanes are set at spawn.
    fn dust_material(&self, palette: Handle<Image>) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                // Moderate sharpness for soft dissolve edges; look().dust_alpha overall.
                fade: Vec4::new(0.0, look().dust_sharpness, 0.0, look().dust_alpha),
                glow: Vec4::new(0.0, 0.0, soft_particle_depth(), 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: palette,
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// The ping material template: additive glow (glow.y = 1 — premultiplied additive), single-frame
    /// full-image round glow, hard erosion so it snaps rather than ghosts.
    fn ping_material(&self) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 1.0, 1.0, 0.0),
                fade: Vec4::new(0.0, look().spark_sharpness, 0.0, 1.0),
                glow: Vec4::new(look().ping_glow, 1.0, 0.0, 0.0),
            },
            atlas: self.ping_atlas.clone(),
            lut: self.spark_lut.clone(),
            alpha_mode: AlphaMode::Add,
        }
    }

    /// The spark material template: additive (hot metal never darkens), a 2×2 tapered-streak atlas
    /// (each spark picks a random frame), hard erosion edge so the streak dissolves crisply.
    pub(super) fn spark_material(&self) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, look().spark_sharpness, 0.0, 1.0),
                glow: Vec4::new(look().spark_glow, 1.0, 0.0, 0.0),
            },
            atlas: self.spark_atlas.clone(),
            lut: self.spark_lut.clone(),
            alpha_mode: AlphaMode::Add,
        }
    }

    /// A dirt-mass material for the layered splash's plume/ring/cloud: the dust atlas recolored through
    /// the resolved powder palette, alpha-blend occluding mass (glow.y = 0 — never additive), overall
    /// `alpha` per layer. Shares the already-warmed Blend billboard pipeline.
    fn dirt_material(&self, alpha: f32, palette: Handle<Image>) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, look().powder_sharpness, 0.0, alpha),
                glow: Vec4::new(0.0, 0.0, soft_particle_depth(), 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: palette,
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// The surface-aligned powder front has no soft-particle depth fade: its whole quad lies
    /// along the surface, so a volume-intersection fade would erase the entire front.
    fn shock_ring_material(&self, palette: Handle<Image>) -> BillboardLook {
        let mut material = self.dirt_material(look().splash_ring_alpha, palette);
        material.params.fade.y = look().splash_ring_sharpness;
        material.params.glow.z = 0.0;
        material
    }

    /// The flat ground scar: the struck ground's own soil palette lying on the surface, with no
    /// soft-particle depth because its whole quad lies inside that fade band.
    fn ground_mark_material(&self, palette: Handle<Image>) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(3.0, 2.0, 2.0, 0.0),
                fade: look().scar_fade,
                glow: Vec4::ZERO,
            },
            atlas: self.dust_atlas.clone(),
            lut: palette,
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// The dirt-ejecta material: the spark-streak geometry recolored to dirt (Blend, no glow — soil
    /// clods are lit mass, not hot metal). Same Blend pipeline as the dust masses.
    fn ejecta_material(&self, palette: Handle<Image>) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, look().spark_sharpness, 0.0, 1.0),
                glow: Vec4::new(0.0, 0.0, 0.0, 0.0),
            },
            atlas: self.spark_atlas.clone(),
            lut: palette,
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// Alpha-blended surface powder in its resolved substance colour.
    fn spall_material(&self, alpha: f32, feedback: SurfaceFeedback) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, look().spall_sharpness, 0.0, alpha),
                glow: Vec4::new(0.0, 0.0, soft_particle_depth(), 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: self.powder_lut(feedback),
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// The armor contact flash: a white-hot additive bloom, hotter/whiter than the compact spark's
    /// warm flash ([`ImpactAssets::ping_material`] on the spark LUT) — welding-bright steel contact.
    /// The round-glow atlas through the white-hot `hot_lut`.
    fn armor_flash_material(&self) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 1.0, 1.0, 0.0),
                fade: Vec4::new(0.0, look().spark_sharpness, 0.0, 1.0),
                glow: Vec4::new(look().armor_flash_glow, 1.0, 0.0, 0.0),
            },
            atlas: self.ping_atlas.clone(),
            lut: self.hot_lut.clone(),
            alpha_mode: AlphaMode::Add,
        }
    }

    /// The armor spark material: the spark-streak atlas through the white-hot `hot_lut` — the dense
    /// fan's hotter, whiter streak (the terrain `spark_material` cools to ember orange). Additive.
    fn armor_spark_material(&self) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, look().spark_sharpness, 0.0, 1.0),
                glow: Vec4::new(look().armor_spark_glow, 1.0, 0.0, 0.0),
            },
            atlas: self.spark_atlas.clone(),
            lut: self.hot_lut.clone(),
            alpha_mode: AlphaMode::Add,
        }
    }

    /// The flame-lick material (penetration only): a brief warm additive flame off the breach — the
    /// spark LUT's orange tail read as fire, over the soft dust atlas so it reads as a lick, not a
    /// spark. Additive so it blooms; kept small and short (see the flame-lick constants).
    fn flame_material(&self) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, look().flash_sharpness, 0.0, 1.0),
                glow: Vec4::new(look().flame_lick_glow, 1.0, 0.0, 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: self.spark_lut.clone(),
            alpha_mode: AlphaMode::Add,
        }
    }
}

pub(super) fn setup_impact_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Spark LUT: white-yellow hot at birth cooling toward ember orange, heat front-loaded so young
    // sparks bloom and old ones die as dim embers. Shared by the ping (a hot round glow).
    let spark_lut = gradient_lut(&mut images, |x, y| {
        let cool = 1.0 - y;
        let color = LinearRgba::rgb(
            x,
            x * (look().spark_green.0 + look().spark_green.1 * cool),
            x * x * (look().spark_blue.0 + look().spark_blue.1 * cool),
        );
        (
            color,
            x * (look().spark_heat.0 + look().spark_heat.1 * cool),
        )
    });
    let mut powder_luts = BTreeMap::new();
    for feedback in crate::substances::SubstanceRegistry::shipped_static().feedbacks() {
        powder_luts.entry(feedback.powder).or_insert_with(|| {
            let [r, g, b] = feedback.powder_linear();
            gradient_lut(&mut images, |x, _| {
                let lum = look().powder_luma.0 + look().powder_luma.1 * x;
                (LinearRgba::rgb(r * lum, g * lum, b * lum), 0.0)
            })
        });
    }
    // Hot LUT for the armor flash + spark fan: white-hot at birth (all channels high, a touch of blue
    // in the hottest core) cooling toward white-yellow — hotter and WHITER than the spark LUT's orange
    // ember tail (armor contact is a welding-bright flash, not a campfire ember). Heat front-loaded.
    let hot_lut = gradient_lut(&mut images, |x, y| {
        let cool = 1.0 - y;
        let color = LinearRgba::rgb(
            x,
            x * (look().hot_green.0 + look().hot_green.1 * cool),
            x * (look().hot_blue.0 + look().hot_blue.1 * cool),
        );
        (color, x * (look().hot_heat.0 + look().hot_heat.1 * cool))
    });
    commands.insert_resource(ImpactAssets {
        quad: unit_quad(&mut meshes),
        dust_atlas: asset_server.load(super::Texture::ImpactDust.path()),
        // A generic round glow (the same `light_01`-derived sprite the MG core uses; the asset
        // server dedupes the load and shares the GPU texture).
        ping_atlas: asset_server.load(super::Texture::MgCore.path()),
        spark_atlas: asset_server.load(super::Texture::Spark.path()),
        spark_lut,
        powder_luts,
        hot_lut,
        ground_palettes: super::muzzle::DustPalettes::default(),
    });
}

#[derive(SystemParam)]
pub(super) struct ImpactKit<'w, 's> {
    assets: ResMut<'w, ImpactAssets>,
    materials: BillboardMaterials<'w>,
    ring: ResMut<'w, BillboardRing>,
    ground_ring: ResMut<'w, GroundMarkRing>,
    rng: ResMut<'w, ViewRng>,
    /// Absent in undrawn worlds, which have no painted ground to lift.
    ground_types: Option<Res<'w, crate::ground_types::GroundTypes>>,
    painted: Option<Res<'w, crate::track_prints::PrintDepths>>,
    images: Option<ResMut<'w, Assets<Image>>>,
    pub(super) commands: Commands<'w, 's>,
}

/// The game's damage-model view resolves surface facts before choosing emission layers.
/// Viewer position supplies orientation only; every dimension comes from the contact recipe.
fn spawn_impact_read(
    impact: On<Impact>,
    mut kit: ImpactKit,
    camera: Query<(&GlobalTransform, Option<&Exposure>), With<PlayerView>>,
) {
    let normal = *Dir3::new(impact.normal)
        .expect("the resolved impact carries a finite nonzero surface normal");
    let (to_camera, exposure) = if camera.is_empty() {
        (Vec3::Z, Exposure::default()) // An undrawn effect harness has no viewer.
    } else {
        let (eye, exposure) = camera
            .single()
            .expect("exactly one camera declares PlayerView");
        // Bevy 0.19 camera extraction uses its default exposure when the component is absent.
        (
            eye.translation() - impact.position,
            exposure.copied().unwrap_or_default(),
        )
    };
    match impact.surface {
        ImpactSurface::Terrain => {
            let (table, painted) = match (kit.ground_types.as_ref(), kit.painted.as_ref()) {
                (Some(table), Some(painted)) => (table, painted),
                (None, None) => return, // Undrawn worlds have no ground response and emit no dust.
                _ => panic!("drawn terrain installs its ground table and painted mask together"),
            };
            let weights = painted.ground_weights(impact.position.xz());
            let sources = crate::terrain_blend::dust_sources(table, weights);
            for (amount, soil) in sources {
                if amount == 0.0 {
                    continue;
                } // This source contributes no lifted material.
                let images = kit
                    .images
                    .as_mut()
                    .expect("a drawn ground has an image asset store");
                let palette = kit.assets.ground_palettes.palette(images, soil.colour_srgb);
                spawn_powder_read(
                    impact.position,
                    normal,
                    to_camera,
                    impact.style,
                    PowderRead {
                        palette,
                        amount,
                        light: crate::world::level_ground_irradiance_lux() / std::f32::consts::PI
                            * exposure.exposure(),
                    },
                    &mut kit,
                );
            }
        }
        ImpactSurface::Material(feedback) => spawn_material_read(
            impact.position,
            normal,
            to_camera,
            impact.style,
            impact.outcome,
            feedback,
            &mut kit,
        ),
    }
}

struct PowderRead {
    palette: Handle<Image>,
    /// Fraction of the recipe's lifted mass supplied by this material, in [0,1].
    amount: f32,
    light: f32,
}

impl PowderRead {
    fn material(&self, mut material: BillboardLook) -> BillboardLook {
        material.params.fade.w *= self.amount;
        self.illuminate(material)
    }

    fn illuminate(&self, mut material: BillboardLook) -> BillboardLook {
        material.params.glow.x = self.light - 1.0;
        material
    }
}

/// The same powder renderer serves resolved ground fines and hard-surface powder.
fn spawn_powder_read(
    position: Vec3,
    normal: Vec3,
    to_camera: Vec3,
    style: ImpactStyle,
    powder: PowderRead,
    kit: &mut ImpactKit,
) {
    if style == ImpactStyle::Layered {
        spawn_big_splash(position, normal, to_camera, powder, kit);
    } else {
        spawn_small_impact(position, normal, to_camera, powder, false, kit);
    }
}

/// The resolved substance chooses its emission response. Non-sparking matter produces its own small
/// powder read under every style: ADR-0023 keeps the plume, the shock ring and the brown cloud off
/// everything but soil.
pub(super) fn spawn_material_read(
    position: Vec3,
    normal: Vec3,
    to_camera: Vec3,
    style: ImpactStyle,
    outcome: ImpactOutcome,
    feedback: SurfaceFeedback,
    kit: &mut ImpactKit,
) {
    if feedback.sparks && style == ImpactStyle::Layered {
        spawn_sparking_read(position, normal, to_camera, outcome, feedback, kit);
    } else {
        let powder = PowderRead {
            palette: kit.assets.powder_lut(feedback),
            amount: 1.0,
            light: 1.0,
        };
        spawn_small_impact(position, normal, to_camera, powder, feedback.sparks, kit);
    }
}

/// Powder opacity follows the resolved lifted share. Only a sparking substance adds hot layers.
fn spawn_small_impact(
    position: Vec3,
    normal: Vec3,
    to_camera: Vec3,
    powder: PowderRead,
    sparks: bool,
    kit: &mut ImpactKit,
) {
    let ImpactKit {
        assets,
        materials,
        ring,
        rng,
        commands,
        ..
    } = kit;
    let billow = powder.material(assets.dust_material(powder.palette.clone()));

    // --- Dust billow: one alpha-blended eroding billboard, random frame/roll/spin, blooming and
    // drifting out along the surface normal (with a gentle rise).
    let dust_size = rng.range(look().dust_size.0, look().dust_size.1);
    spawn_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: billow,
            lifetime: rng.range(look().dust_lifetime.0, look().dust_lifetime.1),
            origin: position + normal * (look().dust_size.0 * 0.5),
            drift: normal * look().dust_normal_push + Vec3::Y * look().dust_rise,
            frames: 4,
            flipbook: Flipbook::Loop {
                start: rng.range(0.0, 4.0),
                rate: look().dust_frame_rate,
            },
            start_size: look().dust_size.0,
            end_size: dust_size,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: rng.range(-look().dust_spin_max, look().dust_spin_max),
            erosion_end: 1.0,
            rotation: None,
        },
    );

    if !sparks {
        return; // The substance states no contact-spark emission.
    }

    // --- Ping: one small additive round glow at the hit point, 1–2 frames — the instant of contact.
    spawn_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.ping_material(),
            lifetime: look().ping_lifetime,
            origin: position + normal * look().surface_clearance_m,
            drift: Vec3::ZERO,
            frames: 1,
            flipbook: Flipbook::Loop {
                start: 0.0,
                rate: 0.0,
            },
            start_size: look().ping_size,
            end_size: look().ping_size * look().ping_end_scale,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: 0.0,
            erosion_end: 0.0,
            rotation: None,
        },
    );

    // Spark orientation is fixed along each launch direction; the palette cools with age.
    let (tan_a, tan_b) = normal.any_orthonormal_pair();
    let count = look().spark_count.0
        + (rng.next_f32() * (look().spark_count.1 - look().spark_count.0 + 1) as f32) as u32;
    for _ in 0..count.min(look().spark_count.1) {
        let theta = rng.range(0.0, std::f32::consts::TAU);
        let spread = rng.range(look().spark_spread.0, look().spark_spread.1);
        let dir = (normal + (tan_a * theta.cos() + tan_b * theta.sin()) * spread).normalize();
        let speed = rng.range(look().spark_speed.0, look().spark_speed.1);
        let length = speed * rng.range(look().spark_stretch.0, look().spark_stretch.1);
        spawn_billboard(
            commands,
            materials,
            ring,
            assets.quad.clone(),
            BillboardSpec {
                material: assets.spark_material(),
                lifetime: rng.range(look().spark_lifetime.0, look().spark_lifetime.1),
                origin: position + dir * (length * 0.5),
                drift: dir * speed,
                frames: 4,
                flipbook: Flipbook::Loop {
                    start: rng.range(0.0, 4.0),
                    rate: 0.0,
                },
                start_size: length,
                end_size: length * look().spark_end_scale,
                aspect: Vec3::new(look().spark_width_ratio, 1.0, 1.0),
                roll: 0.0,
                spin: 0.0,
                erosion_end: 1.0,
                rotation: Some(spark_orientation(dir, to_camera)),
            },
        );
    }
}

/// The layered soil strike from one resolved ground contribution: ejecta, a tall plume, a shock
/// ring and a lingering cloud on the shared ring, plus one flat scar in its own ring. Physical
/// scale, never screen-relative.
fn spawn_big_splash(
    position: Vec3,
    normal: Vec3,
    to_camera: Vec3,
    powder: PowderRead,
    kit: &mut ImpactKit,
) {
    let ImpactKit {
        assets,
        materials,
        ring,
        ground_ring,
        rng,
        commands,
        ..
    } = kit;
    let (tan_a, tan_b) = normal.any_orthonormal_pair();

    // --- Ejecta streaks: soil clods thrown in an up-biased cone (blend the normal toward straight up
    // by look().ejecta_up_bias, then spread), each a dirt-recolored stretched streak launched at speed and
    // falling under look().debris_gravity. The streak's ORIENTATION is baked at spawn from the launch
    // direction, so it holds the ballistic-initial heading for its ~0.3 s life.
    // DERIVED: bias > 1/2 bounds the length below by 2*bias - 1, even for opposite unit inputs.
    let up_axis =
        *Dir3::new(normal * (1.0 - look().ejecta_up_bias) + Vec3::Y * look().ejecta_up_bias)
            .expect("the upward bias of unit directions defines a finite nonzero ejecta axis");
    let count = look().ejecta_count.0
        + (rng.next_f32() * (look().ejecta_count.1 - look().ejecta_count.0 + 1) as f32) as u32;
    for _ in 0..((count.min(look().ejecta_count.1) as f32 * powder.amount).round() as u32) {
        let theta = rng.range(0.0, std::f32::consts::TAU);
        let spread = rng.range(look().spark_spread.0, look().spark_spread.1);
        let dir = (up_axis + (tan_a * theta.cos() + tan_b * theta.sin()) * spread).normalize();
        let speed = rng.range(look().ejecta_speed.0, look().ejecta_speed.1);
        let length = speed * rng.range(look().ejecta_stretch.0, look().ejecta_stretch.1);
        spawn_ballistic_billboard(
            commands,
            materials,
            ring,
            assets.quad.clone(),
            BillboardSpec {
                material: powder.illuminate(assets.ejecta_material(powder.palette.clone())),
                lifetime: rng.range(look().ejecta_lifetime.0, look().ejecta_lifetime.1),
                origin: position + dir * (length * 0.5),
                drift: dir * speed,
                frames: 4,
                flipbook: Flipbook::Loop {
                    start: rng.range(0.0, 4.0),
                    rate: 0.0,
                },
                start_size: length,
                end_size: length * look().ejecta_end_scale,
                aspect: Vec3::new(look().ejecta_width_ratio, 1.0, 1.0),
                roll: 0.0,
                spin: 0.0,
                erosion_end: 1.0,
                rotation: Some(spark_orientation(dir, to_camera)),
            },
            look().debris_gravity,
        );
    }

    // --- Dirt plume: 1–2 tall camera-facing dirt columns rising off the strike, blooming to full
    // height over their life then eroding out.
    let plumes = look().splash_plume_count.0
        + (rng.next_f32() * (look().splash_plume_count.1 - look().splash_plume_count.0 + 1) as f32)
            as u32;
    for _ in 0..plumes.min(look().splash_plume_count.1) {
        spawn_billboard(
            commands,
            materials,
            ring,
            assets.quad.clone(),
            BillboardSpec {
                material: powder
                    .material(assets.dirt_material(look().plume_alpha, powder.palette.clone())),
                lifetime: look().splash_plume_lifetime,
                origin: position + normal * (look().splash_plume_size.0 * 0.5),
                drift: Vec3::Y * look().splash_plume_rise,
                frames: 4,
                flipbook: Flipbook::Loop {
                    start: rng.range(0.0, 4.0),
                    rate: 0.0,
                },
                start_size: look().splash_plume_size.0,
                end_size: look().splash_plume_size.1,
                aspect: look().splash_plume_aspect,
                roll: rng.range(-look().plume_roll_rad, look().plume_roll_rad),
                spin: 0.0,
                erosion_end: 1.0,
                rotation: None,
            },
        );
    }

    // --- Ground shock ring: a hard-edged, ground-oriented front racing outward flat off the strike
    // and gone well before the fountain lands — the powder front, distinct from the lingering haze below.
    spawn_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: powder.material(assets.shock_ring_material(powder.palette.clone())),
            lifetime: look().splash_ring_lifetime,
            origin: position + normal * look().flash_clearance_m,
            drift: Vec3::ZERO,
            frames: 4,
            flipbook: Flipbook::Loop {
                start: rng.range(0.0, 4.0),
                rate: 0.0,
            },
            start_size: look().splash_ring_size.0,
            end_size: look().splash_ring_size.1,
            aspect: Vec3::ONE,
            roll: 0.0,
            spin: 0.0,
            erosion_end: 1.0,
            rotation: Some(ground_orientation(
                normal,
                rng.range(0.0, std::f32::consts::TAU),
            )),
        },
    );

    // --- Lingering cloud: the large, low-alpha brown haze that hangs after the fountain falls,
    // spreading and thinning for seconds after every other layer has ended.
    spawn_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: powder
                .material(assets.dirt_material(look().splash_cloud_alpha, powder.palette.clone())),
            lifetime: look().splash_cloud_lifetime,
            origin: position
                + normal * (look().splash_cloud_size.0 * 0.5)
                + Vec3::Y * look().cloud_lift_m,
            drift: Vec3::Y * look().splash_cloud_rise,
            frames: 4,
            flipbook: Flipbook::Loop {
                start: rng.range(0.0, 4.0),
                rate: 0.0,
            },
            start_size: look().splash_cloud_size.0,
            end_size: look().splash_cloud_size.1,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: 0.0,
            erosion_end: 1.0,
            rotation: None,
        },
    );

    // --- Ground scar: one flat patch of the turned-up soil, its opacity the lifted share and its
    // colour the ground's own, darker by `scar_shade` than the soil in the air.
    let mut scar = powder.material(assets.ground_mark_material(powder.palette.clone()));
    scar.params.glow.x = powder.light * look().scar_shade - 1.0;
    spawn_billboard(
        commands,
        materials,
        &mut ground_ring.0,
        assets.quad.clone(),
        BillboardSpec {
            material: scar,
            lifetime: look().ground_mark_lifetime,
            origin: position + normal * look().surface_clearance_m,
            drift: Vec3::ZERO,
            frames: 4,
            flipbook: Flipbook::Loop {
                start: 3.0,
                rate: 0.0,
            },
            start_size: look().ground_mark_size,
            end_size: look().ground_mark_size * look().scar_end_scale,
            aspect: Vec3::ONE,
            roll: 0.0,
            spin: 0.0,
            erosion_end: 1.0,
            rotation: Some(ground_orientation(
                normal,
                rng.range(0.0, std::f32::consts::TAU),
            )),
        },
    );
}

/// The large sparking-surface recipe: contact glow, a spark fan and the substance's powder colour.
/// A resolved bounce supplies the fan's bearing; an entered face admits a brief warm lick. Its
/// emission geometry remains provisional. It never writes a soil scar or changes gameplay state.
fn spawn_sparking_read(
    position: Vec3,
    normal: Vec3,
    to_camera: Vec3,
    outcome: ImpactOutcome,
    feedback: SurfaceFeedback,
    kit: &mut ImpactKit,
) {
    let ImpactKit {
        assets,
        materials,
        ring,
        rng,
        commands,
        ..
    } = kit;
    // The spark fan's axis: straight off the surface normal, or leaned toward the deflected outgoing
    // direction on a ricochet (a bounce throws its sparks the way it kicked off).
    let axis = match outcome.deflection() {
        Some(deflection) => {
            let defl = *Dir3::new(deflection)
                .expect("a resolved bounce carries a finite nonzero outgoing direction");
            // DERIVED: bias > 1/2 prevents two unit terms from cancelling.
            *Dir3::new(
                normal * (1.0 - look().armor_spark_deflect_bias)
                    + defl * look().armor_spark_deflect_bias,
            )
            .expect("the deflection bias defines a finite nonzero spark axis")
        }
        None => normal, // A non-bounce has no outgoing bearing; its fan follows the surface normal.
    };
    let (tan_a, tan_b) = axis.any_orthonormal_pair();

    // --- White-hot contact flash: one big hot additive bloom at the strike, gone in a frame or two.
    spawn_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.armor_flash_material(),
            lifetime: look().armor_flash_lifetime,
            origin: position + normal * look().flash_clearance_m,
            drift: Vec3::ZERO,
            frames: 1,
            flipbook: Flipbook::Loop {
                start: 0.0,
                rate: 0.0,
            },
            start_size: look().armor_flash_size,
            end_size: look().armor_flash_size * look().flash_end_scale,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: 0.0,
            erosion_end: 0.0,
            rotation: None,
        },
    );

    // --- Dense hot spark fan: many fast white-hot streaks in a cone around the fan axis, each a
    // fixed-orientation billboard elongated along its own flight direction and falling under
    // look().debris_gravity (the orientation is baked at spawn, so a streak holds its launch heading).
    let count = look().armor_spark_count.0
        + (rng.next_f32() * (look().armor_spark_count.1 - look().armor_spark_count.0 + 1) as f32)
            as u32;
    for _ in 0..count.min(look().armor_spark_count.1) {
        let theta = rng.range(0.0, std::f32::consts::TAU);
        let spread = rng.range(look().armor_spark_spread.0, look().armor_spark_spread.1);
        let dir = (axis + (tan_a * theta.cos() + tan_b * theta.sin()) * spread).normalize();
        let speed = rng.range(look().armor_spark_speed.0, look().armor_spark_speed.1);
        let length = speed * rng.range(look().armor_spark_stretch.0, look().armor_spark_stretch.1);
        spawn_ballistic_billboard(
            commands,
            materials,
            ring,
            assets.quad.clone(),
            BillboardSpec {
                material: assets.armor_spark_material(),
                lifetime: rng.range(look().armor_spark_lifetime.0, look().armor_spark_lifetime.1),
                origin: position + dir * (length * 0.5),
                drift: dir * speed,
                frames: 4,
                flipbook: Flipbook::Loop {
                    start: rng.range(0.0, 4.0),
                    rate: 0.0,
                },
                start_size: length,
                end_size: length * look().spark_end_scale,
                aspect: Vec3::new(look().armor_spark_width_ratio, 1.0, 1.0),
                roll: 0.0,
                spin: 0.0,
                erosion_end: 1.0,
                rotation: Some(spark_orientation(dir, to_camera)),
            },
            look().debris_gravity,
        );
    }

    // --- Small gray spall puff: one short gray smoke mass off the strike (steel spall, never dirt),
    // pushed gently out along the normal, rising, then settling down the plate under look().debris_gravity.
    let puff_size = rng.range(look().spall_puff_size.0, look().spall_puff_size.1);
    spawn_ballistic_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.spall_material(look().spall_puff_alpha, feedback),
            lifetime: rng.range(look().spall_puff_lifetime.0, look().spall_puff_lifetime.1),
            origin: position + normal * (look().spall_puff_size.0 * 0.5),
            drift: normal * look().spall_push_mps + Vec3::Y * look().spall_puff_rise,
            frames: 4,
            flipbook: Flipbook::Loop {
                start: rng.range(0.0, 4.0),
                rate: 0.0,
            },
            start_size: look().spall_puff_size.0,
            end_size: puff_size,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: rng.range(-look().dust_spin_max, look().dust_spin_max),
            erosion_end: 1.0,
            rotation: None,
        },
        look().debris_gravity,
    );

    // --- Flame lick: penetration ONLY (the round bit into the steel — embed-that-defeats or a clean
    // perforation, never a ricochet). One brief warm additive flame off the breach, rising. Full
    // width on frame one; it collapses rather than blooms (the burn is at the instant of the bite).
    if outcome.verdict().entered() {
        spawn_billboard(
            commands,
            materials,
            ring,
            assets.quad.clone(),
            BillboardSpec {
                material: assets.flame_material(),
                lifetime: look().flame_lick_lifetime,
                origin: position + normal * (look().flame_lick_size.1 * 0.5),
                drift: normal * look().flame_push_mps + Vec3::Y * look().flame_lick_rise,
                frames: 4,
                flipbook: Flipbook::Loop {
                    start: rng.range(0.0, 4.0),
                    rate: 0.0,
                },
                start_size: look().flame_lick_size.0,
                end_size: look().flame_lick_size.1,
                aspect: Vec3::ONE,
                roll: rng.range(0.0, std::f32::consts::TAU),
                spin: 0.0,
                erosion_end: 1.0,
                rotation: None,
            },
        );
    }
}

/// Orientation for a flat, ground-lying billboard: turn the quad's +Z (its face normal) onto the
/// surface `normal` so it lies ON the terrain (not camera-facing), then roll it by `roll` around
/// that normal for per-instance variety. `normal` must be unit-length.
fn ground_orientation(normal: Vec3, roll: f32) -> Quat {
    Quat::from_rotation_arc(Vec3::Z, normal) * Quat::from_rotation_z(roll)
}

/// Orientation for a stretched spark: quad +Y along the flight direction, quad normal (+Z) turned
/// as close to the camera as that alignment allows — the survey's "aligned to velocity AND facing
/// camera" (trick 8). Computed once at spawn: a spark lives ~0.15 s, the camera doesn't move
/// enough to matter. `dir` must be unit-length.
fn spark_orientation(dir: Vec3, to_camera: Vec3) -> Quat {
    assert!(
        dir.is_finite() && to_camera.is_finite(),
        "spark orientation inputs are finite"
    );
    // The camera direction's component perpendicular to the streak axis; parallel/coincident views
    // fall back to any perpendicular (the streak is then seen end-on — invisible either way).
    let z = (to_camera - dir * to_camera.dot(dir))
        .try_normalize()
        .unwrap_or_else(|| dir.any_orthonormal_vector());
    // Right-handed basis: X = Y × Z.
    Quat::from_mat3(&Mat3::from_cols(dir.cross(z), dir, z))
}

#[cfg(test)]
mod tests {
    use super::*;

    use bevy::render::storage::ShaderBuffer;

    use bevy::mesh::MeshTag;

    use crate::vfx::billboard::{
        BILLBOARD_SLOTS, Billboard, BillboardFamilies, VfxBillboardMaterial,
    };

    /// FIXTURE DATA: the compact contact recipe.
    const COMPACT: ImpactStyle = ImpactStyle::Compact;
    /// FIXTURE DATA: the layered contact recipe.
    const LAYERED: ImpactStyle = ImpactStyle::Layered;

    /// Minimal app carrying what the impact observer reads. Real `Assets` stores (initialized bare,
    /// no asset plugins) so the family materials and the lane table are made for real; fixed-seed
    /// view RNG; no camera (spark facing falls back).
    fn harness() -> App {
        let mut app = App::new();
        app.init_resource::<BillboardRing>()
            .init_resource::<GroundMarkRing>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<VfxBillboardMaterial>>()
            .init_resource::<Assets<ShaderBuffer>>()
            .init_resource::<BillboardFamilies>()
            .init_resource::<Time>()
            .insert_resource(ViewRng::seeded(42))
            .add_observer(spawn_impact_read);
        app.insert_resource(ImpactAssets::test_stub())
            .insert_resource(crate::ground_types::GroundTypes::load_shipped())
            .insert_resource(crate::track_prints::PrintDepths::unpainted());
        app
    }

    /// Fixture terrain contact, using the painted ground installed by the harness.
    fn trigger_impact(app: &mut App, normal: Vec3, style: ImpactStyle) {
        trigger_surface(
            app,
            normal,
            style,
            ImpactSurface::Terrain,
            ImpactOutcome::Struck,
        );
    }

    /// Fire an impact on an explicit surface (with verdict + deflection) — the armor tests.
    fn trigger_surface(
        app: &mut App,
        normal: Vec3,
        style: ImpactStyle,
        surface: ImpactSurface,
        outcome: ImpactOutcome,
    ) {
        app.world_mut().trigger(Impact {
            position: Vec3::ZERO,
            normal,
            style,
            surface,
            outcome,
        });
        app.world_mut().flush();
    }

    fn billboards(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<Billboard>>()
            .iter(app.world())
            .count()
    }

    /// The game's damage-model view must emit only dust the resolved ground gives up.
    /// FIXTURE: every ground retains its fines; a fixed terrain recipe invents a powder cloud.
    #[test]
    fn non_erodible_ground_emits_no_powder() {
        let mut app = harness();
        let mut table = crate::ground_types::GroundTypes::load_shipped();
        for ground in table.types.values_mut() {
            ground.dust.erodibility = 0.0;
        }
        app.insert_resource(table)
            .insert_resource(crate::track_prints::PrintDepths::unpainted());
        trigger_impact(&mut app, Vec3::Y, LAYERED);
        assert_eq!(billboards(&mut app), 0, "no ground material was lifted");
    }

    /// The game's damage-model view consumes the ground's own colour and lifted share.
    /// FIXTURE: vary all grounds together so this test owns no terrain pack's identity.
    #[test]
    fn ground_powder_density_and_colour_follow_the_authored_response() {
        for (amount, colour) in [(0.25, [0.2, 0.3, 0.4]), (1.0, [0.6, 0.4, 0.2])] {
            let mut app = harness();
            let mut table = crate::ground_types::GroundTypes::load_shipped();
            for ground in table.types.values_mut() {
                ground.dust.erodibility = amount;
                ground.dust.colour_srgb = colour;
            }
            app.insert_resource(table);
            trigger_impact(&mut app, Vec3::Y, COMPACT);
            let world = app.world_mut();
            let (billboard, material) = world
                .query::<(&Billboard, &MeshMaterial3d<VfxBillboardMaterial>)>()
                .single(world)
                .expect("one compact powder read and no hot layers");
            assert_eq!(billboard.lanes.fade.w, look().dust_alpha * amount);
            let family = world
                .resource::<Assets<VfxBillboardMaterial>>()
                .get(&material.0)
                .unwrap();
            let image = world.resource::<Assets<Image>>().get(&family.lut).unwrap();
            let data = image.data.as_ref().unwrap();
            let texel = (image.texture_descriptor.size.width as usize - 1) * 4;
            let albedo = LinearRgba::from(Color::srgb_from_array(colour));
            let sun = crate::world::sun_light().color.to_linear();
            let expected = [
                albedo.red * sun.red,
                albedo.green * sun.green,
                albedo.blue * sun.blue,
            ]
            .map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
            assert_eq!(&data[texel..texel + 3], &expected);
            let light = crate::world::level_ground_irradiance_lux() / std::f32::consts::PI
                * Exposure::default().exposure();
            assert!((billboard.lanes.glow.x + 1.0 - light).abs() < f32::EPSILON * light.max(1.0));
        }
    }

    /// The game's damage-model view never inflates a contact for a distant observer.
    /// FIXTURE camera distances change only facing, leaving world dimensions and lifetimes equal.
    #[test]
    fn powder_world_scale_is_independent_of_viewer_distance() {
        let mut reads = Vec::new();
        for distance in [1.0, 10_000.0] {
            let mut app = harness();
            app.world_mut().spawn((
                PlayerView,
                GlobalTransform::from_translation(Vec3::Z * distance),
            ));
            trigger_impact(&mut app, Vec3::Y, LAYERED);
            let world = app.world_mut();
            reads.push(
                world
                    .query::<&Billboard>()
                    .iter(world)
                    .map(|b| (b.start_size, b.end_size, b.lifetime, b.drift, b.accel))
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(reads[0], reads[1]);
    }

    /// The game's damage-model view spends a lifted share once: fewer solid pieces, while each
    /// piece keeps its opacity. FIXTURE: two erodibilities with identical material and view seed.
    #[test]
    fn lifted_solid_count_changes_without_fading_each_piece() {
        let mut reads = Vec::new();
        for amount in [0.25, 1.0] {
            let mut app = harness();
            let mut table = crate::ground_types::GroundTypes::load_shipped();
            for ground in table.types.values_mut() {
                ground.dust.erodibility = amount;
            }
            app.insert_resource(table);
            trigger_impact(&mut app, Vec3::Y, LAYERED);
            let world = app.world_mut();
            let opacities = world
                .query::<&Billboard>()
                .iter(world)
                .filter(|b| b.accel != Vec3::ZERO)
                .map(|b| b.lanes.fade.w)
                .collect::<Vec<_>>();
            assert!(!opacities.is_empty());
            reads.push(opacities);
        }
        assert!(reads[0].len() < reads[1].len());
        assert!(
            reads
                .iter()
                .flatten()
                .all(|opacity| *opacity == reads[1][0])
        );
    }

    /// The game's damage-model view cannot read a partially removed ground as an undrawn world.
    #[test]
    #[should_panic(expected = "ground table and painted mask together")]
    fn a_partial_painted_ground_is_a_composition_bug() {
        let mut app = harness();
        app.world_mut()
            .remove_resource::<crate::track_prints::PrintDepths>()
            .expect("the fixture starts with a complete painted ground");
        trigger_impact(&mut app, Vec3::Y, LAYERED);
    }

    /// Two declared player views are a composition bug. Replacing that state with an arbitrary
    /// facing vector would silently render the impact for neither declared viewer.
    #[test]
    #[should_panic(expected = "exactly one camera declares PlayerView")]
    fn duplicate_player_views_cannot_pick_an_impact_orientation() {
        let mut app = harness();
        for _ in 0..2 {
            app.world_mut()
                .spawn((PlayerView, GlobalTransform::IDENTITY));
        }
        trigger_impact(&mut app, Vec3::Y, LAYERED);
    }

    /// A non-sparking substance produces one small powder read under a Layered round. A class-based
    /// metal recipe would add a flash and sparks, the soil recipe a plume, a ring and a cloud; the
    /// resolved material states none of them, and ADR-0023 relies on it.
    #[test]
    fn non_sparking_matter_never_gets_the_metal_read() {
        let mut app = harness();
        let feedback = SurfaceFeedback {
            sparks: false,
            ..SurfaceFeedback::fixture()
        };
        trigger_surface(
            &mut app,
            Vec3::Y,
            LAYERED,
            ImpactSurface::Material(feedback),
            ImpactOutcome::Through,
        );
        assert_eq!(
            billboards(&mut app),
            1,
            "one powder mass: no metal flash, spark fan, plume, shock ring or cloud"
        );
    }

    /// The ground scars ride their OWN eviction ring, bounded by GROUND_MARK_CAP, so a storm cannot
    /// evict a fresh multi-second scar within a frame; each live scar holds its own lane-table slot
    /// past the shared ring's. A mutant spawning scars on the shared ring fails here.
    #[test]
    fn ground_scars_are_ring_capped() {
        let mut app = harness();
        for _ in 0..GROUND_MARK_CAP + 8 {
            trigger_impact(&mut app, Vec3::Y, LAYERED);
        }
        let ring = &app.world().resource::<GroundMarkRing>().0;
        assert_eq!(ring.live.len(), GROUND_MARK_CAP, "bounded by their own cap");
        let slots: std::collections::BTreeSet<u32> = ring
            .live
            .iter()
            .map(|&scar| app.world().get::<MeshTag>(scar).expect("a live scar").0)
            .collect();
        assert_eq!(slots.len(), GROUND_MARK_CAP, "one slot per live scar");
        assert!(
            slots
                .iter()
                .all(|&slot| (BILLBOARD_CAP..BILLBOARD_SLOTS).contains(&(slot as usize))),
            "scar slots lie past the shared ring's, inside the lane table"
        );
    }

    /// The game's damage-model view: a layered soil strike's scar is the struck ground's own
    /// colour, darker by `scar_shade`, as opaque as the ground gives up soil; a compact strike and
    /// a substance leave none. ADR-0023's surface evidence relies on this; a mutant with a fixed
    /// earth palette or a constant opacity fails here. FIXTURE: every ground the same soil.
    #[test]
    fn the_ground_scar_takes_the_grounds_resolved_colour_and_share() {
        let colour = [0.6, 0.4, 0.2];
        for amount in [0.25, 1.0] {
            let mut app = harness();
            let mut table = crate::ground_types::GroundTypes::load_shipped();
            for ground in table.types.values_mut() {
                ground.dust.erodibility = amount;
                ground.dust.colour_srgb = colour;
            }
            app.insert_resource(table);
            trigger_impact(&mut app, Vec3::Y, LAYERED);
            let scar = *app
                .world()
                .resource::<GroundMarkRing>()
                .0
                .live
                .front()
                .expect("a layered soil strike leaves one scar");
            let billboard = app.world().get::<Billboard>(scar).expect("a live scar");
            assert_eq!(billboard.lanes.fade.w, look().scar_fade.w * amount);
            let light = crate::world::level_ground_irradiance_lux() / std::f32::consts::PI
                * Exposure::default().exposure();
            assert_eq!(billboard.lanes.glow.x, light * look().scar_shade - 1.0);
            assert_eq!(
                billboard.lanes.glow.z, 0.0,
                "the scar lies in the fade band"
            );
            let material = app
                .world()
                .get::<MeshMaterial3d<VfxBillboardMaterial>>(scar)
                .expect("a live scar's family material");
            let lut = app
                .world()
                .resource::<Assets<VfxBillboardMaterial>>()
                .get(&material.0)
                .expect("the family material exists")
                .lut
                .clone();
            let soil = app
                .world_mut()
                .resource_scope(|world, mut assets: Mut<ImpactAssets>| {
                    assets
                        .ground_palettes
                        .palette(&mut world.resource_mut::<Assets<Image>>(), colour)
                });
            assert_eq!(lut, soil, "the struck ground's own soil palette");
        }
        let mut app = harness();
        trigger_impact(&mut app, Vec3::Y, COMPACT);
        trigger_surface(
            &mut app,
            Vec3::Y,
            LAYERED,
            ImpactSurface::Material(SurfaceFeedback {
                sparks: false,
                ..SurfaceFeedback::fixture()
            }),
            ImpactOutcome::Struck,
        );
        assert!(
            app.world().resource::<GroundMarkRing>().0.live.is_empty(),
            "only a layered soil strike scars"
        );
    }

    /// Resolved surfaces supply a finite nonzero normal. Inventing an up-facing surface would
    /// hide a broken geometry or event invariant from the renderer.
    #[test]
    #[should_panic(expected = "finite nonzero surface normal")]
    fn a_degenerate_impact_normal_is_a_bug() {
        let mut app = harness();
        trigger_impact(&mut app, Vec3::ZERO, LAYERED);
    }

    /// The whole read rides the shared billboard ring: an impact storm is bounded by its cap, not
    /// unbounded entity growth (the same leak bound every other vfx layer pins).
    #[test]
    fn impact_storm_is_ring_capped() {
        let mut app = harness();
        for _ in 0..200 {
            trigger_impact(&mut app, Vec3::Y, COMPACT);
        }
        let ring_len = app.world().resource::<BillboardRing>().live.len();
        assert_eq!(
            billboards(&mut app),
            ring_len,
            "live billboards == ring entries"
        );
        assert!(
            ring_len <= BILLBOARD_CAP,
            "impact storm must stay ring-capped (got {ring_len})"
        );
    }

    /// Penetration (the round bit into the steel) appends exactly the flame lick — one extra
    /// billboard over the otherwise-identical non-penetrating armor read. Both harnesses share the
    /// fixed seed, so the RNG-driven fan count is identical and the delta is purely the flame lick.
    #[test]
    fn armor_penetration_adds_exactly_the_flame_lick() {
        let mut no_pen = harness();
        trigger_surface(
            &mut no_pen,
            Vec3::Y,
            LAYERED,
            ImpactSurface::Material(SurfaceFeedback::fixture()),
            ImpactOutcome::Struck,
        );
        let n0 = billboards(&mut no_pen);

        let mut pen = harness();
        trigger_surface(
            &mut pen,
            Vec3::Y,
            LAYERED,
            ImpactSurface::Material(SurfaceFeedback::fixture()),
            ImpactOutcome::Through,
        );
        let n1 = billboards(&mut pen);

        assert_eq!(
            n1,
            n0 + 1,
            "penetration adds exactly the flame lick, nothing else"
        );
    }
}
