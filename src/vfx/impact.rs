//! The game's damage-model view: impact dressing for [`Impact`], game state only.
//!
//! Invariant (ADR-0014): this module only reads simulation events and spawns transient render
//! entities. Resolved substance responses choose powder and sparks; no visual entity affects game state.
//! Required parameters live in `assets/vfx/presentation.ron`; stand-ins name their missing
//! emission laws. The authored world-space scale is independent of viewer distance.

use super::look::impact as look;
use crate::substances::SurfaceFeedback;

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
/// Its slots follow the shared ring's in the billboard lane table (`BILLBOARD_SLOTS`).
pub(super) const GROUND_MARK_CAP: usize = 16;

/// Independent eviction ring for the long-lived ground scars (see [`GROUND_MARK_CAP`]). Insulated
/// from the shared [`BillboardRing`] so an MG storm filling that ring can't evict a fresh layered scar.
#[derive(Resource)]
pub(super) struct GroundMarkRing(pub BillboardRing);

/// Lane-table slots `BILLBOARD_CAP..BILLBOARD_CAP + GROUND_MARK_CAP`, after the shared ring's.
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

/// Preloaded impact view assets: the shared quad, the three atlases (dust billow, ping glow, spark
/// streaks) and the two LUTs (dust palette, spark palette). `pub(super)` so the prewarm rig can warm
/// the exact material permutations at startup.
#[derive(Resource)]
pub(super) struct ImpactAssets {
    pub(super) quad: Handle<Mesh>,
    dust_atlas: Handle<Image>,
    dust_lut: Handle<Image>,
    ping_atlas: Handle<Image>,
    spark_atlas: Handle<Image>,
    spark_lut: Handle<Image>,
    /// Earthy-brown palette for the layered splash's dirt masses (plume/ring/cloud) and its dirt ejecta.
    dirt_lut: Handle<Image>,
    /// Darker scorched-earth palette for the flat ground scar.
    scar_lut: Handle<Image>,
    /// Neutral gray palette for the armor read's spall/smoke (steel spall, never brown dirt) — the
    /// compact material billow swap and the layered armor puff both use it.
    powder_luts: BTreeMap<[u8; 3], Handle<Image>>,
    /// White-hot palette for the armor read's contact flash + dense spark fan (and the flame lick's
    /// hot core) — whiter/hotter than the terrain `spark_lut`, which cools to ember orange.
    hot_lut: Handle<Image>,
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
            dust_lut: Handle::default(),
            ping_atlas: Handle::default(),
            spark_atlas: Handle::default(),
            spark_lut: Handle::default(),
            dirt_lut: Handle::default(),
            scar_lut: Handle::default(),
            powder_luts: [(SurfaceFeedback::fixture().powder, Handle::default())]
                .into_iter()
                .collect(),
            hot_lut: Handle::default(),
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

    /// The dust-billow material template: alpha-blend mass (glow.y = 0 — the occluding contract),
    /// its own earthy LUT, soft erosion edges. Its 2×2 atlas frame lanes are set on spawn.
    fn dust_material(&self) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                // Moderate sharpness for soft dissolve edges; look().dust_alpha overall.
                fade: Vec4::new(0.0, look().dust_sharpness, 0.0, look().dust_alpha),
                glow: Vec4::new(look().dust_glow, 0.0, soft_particle_depth(), 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: self.dust_lut.clone(),
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
    /// the earthy `dirt_lut`, alpha-blend occluding mass (glow.y = 0 — never additive), overall
    /// `alpha` per layer. Shares the already-warmed Blend billboard pipeline.
    fn dirt_material(&self, alpha: f32) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, look().powder_sharpness, 0.0, alpha),
                glow: Vec4::new(0.0, 0.0, soft_particle_depth(), 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: self.dirt_lut.clone(),
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// The ground shock-ring material: the dirt mass at `splash_ring_alpha` with the erosion lane
    /// sharpened (`splash_ring_sharpness`) so the racing front cuts a hard edge instead of the
    /// soft dust blur the plume and haze want.
    ///
    /// The soft-particle fade the other dirt masses carry is CLEARED here: the ring is a flat quad
    /// laid 0.1 m over the surface, so its whole area sits inside the fade band and the layer would
    /// scale itself out of existence (the same reason the ground scar never carries one — see
    /// [`soft_particle_depth`]).
    fn shock_ring_material(&self) -> BillboardLook {
        let mut material = self.dirt_material(look().splash_ring_alpha);
        material.params.fade.y = look().splash_ring_sharpness;
        material.params.glow.z = 0.0;
        material
    }

    /// The dirt-ejecta material: the spark-streak geometry recolored to dirt (Blend, no glow — soil
    /// clods are lit mass, not hot metal). Same Blend pipeline as the dust masses.
    fn ejecta_material(&self) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, look().spark_sharpness, 0.0, 1.0),
                glow: Vec4::new(0.0, 0.0, 0.0, 0.0),
            },
            atlas: self.spark_atlas.clone(),
            lut: self.dirt_lut.clone(),
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// The ground-scar material: the dust atlas pinned to its dirt frame (BR cell 3 of the 2×2 —
    /// `dirt_03`, via a 4-frame/rate-0/start-3 spec) recolored dark through `scar_lut`, alpha-blend,
    /// slow erosion so the gouge fades over its long life. Ground-oriented at spawn (fixed rotation).
    fn ground_mark_material(&self) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(3.0, 2.0, 2.0, 0.0),
                fade: look().scar_fade,
                glow: Vec4::new(0.0, 0.0, 0.0, 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: self.scar_lut.clone(),
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// A gray spall-smoke mass for the armor read's puff: the dust atlas recolored through the
    /// authored powder palette (never the earthy dirt LUT — steel throws gray spall/smoke, not dirt),
    /// alpha-blend occluding mass, overall `alpha`. Shares the already-warmed Blend pipeline.
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

    /// The armor contact flash: a white-hot additive bloom, hotter/whiter than the terrain splash's
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
    // Dust LUT: warm powder-gray at the bright core cooling to an earthy brown as the cloud ages and
    // thins; a whisper of heat only in the young, bright texels (the impact flash lights it for an
    // instant, then it is inert kicked mass).
    let dust_lut = gradient_lut(&mut images, |x, y| {
        let lum = look().dust_luma.0 + look().dust_luma.1 * x;
        let earth = (1.0 - y) * look().dust_earth;
        let color = LinearRgba::rgb(
            lum * (look().dust_red + earth),
            lum * (look().dust_green.0 + earth * look().dust_green.1),
            lum * (look().dust_blue.0 + earth * look().dust_blue.1),
        );
        let heat = look().dust_heat.0 * x * (-y * look().dust_heat.1).exp();
        (color, heat)
    });
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
    // Dirt LUT for the layered splash masses: an earthy brown, brighter/tanner where the sprite signal is
    // strong (sunlit dirt face) sinking to a dark damp brown in shadow; darkening slightly as the
    // cloud ages (Y). No heat — kicked soil is inert lit mass, never an emitter.
    let dirt_lut = gradient_lut(&mut images, |x, y| {
        let lum = look().dirt_luma.0 + look().dirt_luma.1 * x;
        let age = 1.0 - y;
        let color = LinearRgba::rgb(
            lum * (look().dirt_red.0 + look().dirt_red.1 * age),
            lum * (look().dirt_green.0 + look().dirt_green.1 * age),
            lum * (look().dirt_blue.0 + look().dirt_blue.1 * age),
        );
        (color, 0.0)
    });
    // Scar LUT: the same earth pulled much darker — a scorched, disturbed-earth gouge that reads as a
    // stain on the terrain, not a bright decal. No heat.
    let scar_lut = gradient_lut(&mut images, |x, _| {
        let lum = look().scar_luma.0 + look().scar_luma.1 * x;
        let color = LinearRgba::rgb(
            lum * look().scar_red,
            lum * look().scar_green,
            lum * look().scar_blue,
        );
        (color, 0.0)
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
        dust_lut,
        // A generic round glow (the same `light_01`-derived sprite the MG core uses; the asset
        // server dedupes the load and shares the GPU texture).
        ping_atlas: asset_server.load(super::Texture::MgCore.path()),
        spark_atlas: asset_server.load(super::Texture::Spark.path()),
        spark_lut,
        dirt_lut,
        scar_lut,
        powder_luts,
        hot_lut,
    });
}

/// What every impact read spawns into: its assets, the billboard material store, the shared
/// billboard and ground-mark rings, the view RNG and `Commands`.
#[derive(SystemParam)]
pub(super) struct ImpactKit<'w, 's> {
    assets: Res<'w, ImpactAssets>,
    materials: BillboardMaterials<'w>,
    ring: ResMut<'w, BillboardRing>,
    ground_ring: ResMut<'w, GroundMarkRing>,
    rng: ResMut<'w, ViewRng>,
    pub(super) commands: Commands<'w, 's>,
}

/// Dress the resolved surface's response. Material facts select sparks and powder colour; terrain
/// has its own ground recipe. Dimensions remain the provisional compact/large recipes below,
/// selected by authored presentation data, not derived from deposited energy and emitted material.
fn spawn_impact_read(
    impact: On<Impact>,
    mut kit: ImpactKit,
    camera: Query<&GlobalTransform, With<PlayerView>>,
) {
    let normal = *Dir3::new(impact.normal)
        .expect("the resolved impact carries a finite nonzero surface normal");
    let to_camera = if camera.is_empty() {
        Vec3::Z // A headless effect harness has no viewer; orientation has no visible consequence.
    } else {
        camera
            .single()
            .expect("exactly one camera declares PlayerView")
            .translation()
            - impact.position
    };
    match impact.surface {
        ImpactSurface::Terrain => {
            spawn_terrain_read(impact.position, normal, to_camera, impact.style, &mut kit)
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

/// The authored terrain recipe. `normal` must be unit; `to_camera` need not be.
pub(super) fn spawn_terrain_read(
    position: Vec3,
    normal: Vec3,
    to_camera: Vec3,
    style: ImpactStyle,
    kit: &mut ImpactKit,
) {
    if style == ImpactStyle::Layered {
        spawn_big_splash(position, normal, to_camera, kit);
    } else {
        spawn_small_impact(position, normal, to_camera, ImpactSurface::Terrain, kit);
    }
}

/// The resolved substance chooses its emission response. Non-sparking matter produces its own
/// powder read, never a metal flash or a spark fan. The authored style selects the existing detail
/// recipe after the material's emission has admitted it.
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
        spawn_small_impact(
            position,
            normal,
            to_camera,
            ImpactSurface::Material(feedback),
            kit,
        );
    }
}

/// The compact read (the original terrain body, plus a gray-spall LUT swap on armor):
/// dust billow (mass) + ping (contact flash) + a few sparks (crisp garnish), all on the shared
/// billboard ring. This is what makes the four invisible non-tracer rounds of the belt cycle register
/// at the target. On ARMOR the billow's LUT swaps from the earthy dust palette to the neutral gray
/// authored powder palette, so each substance keeps its own colour; the
/// swap draws NO RNG, so the byte-identical terrain read (and its pinned RNG order) is untouched.
fn spawn_small_impact(
    position: Vec3,
    normal: Vec3,
    to_camera: Vec3,
    surface: ImpactSurface,
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
    // The billow material: the earthy dust palette on terrain, the gray spall palette on armor. Only
    // the LUT differs — same params, same atlas, same Blend pipeline — and choosing it draws no RNG,
    // so the small read's RNG sequence (the pinned draw order) is identical on both surfaces.
    let billow = {
        let mut m = assets.dust_material();
        if let ImpactSurface::Material(feedback) = surface {
            m.lut = assets.powder_lut(feedback);
        }
        m
    };

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

    if matches!(surface, ImpactSurface::Material(feedback) if !feedback.sparks) {
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

    // --- Sparks: 2–4 stretched additive streaks in a cone around the surface normal (a degenerate
    // normal falls back to straight up — sparks off the ground still read). Each is a fixed-
    // orientation billboard elongated along its own flight direction, drifting at launch speed;
    // erosion + the LUT's cooling row kill it as a dim ember.
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

/// The layered terrain recipe.
/// Contact flash → dirt ejecta → tall dirt plume → ground shock ring → lingering cloud on the shared
/// ring, plus one flat ground scar in its own ring. Physical scale (module doc), not screen-relative.
fn spawn_big_splash(position: Vec3, normal: Vec3, to_camera: Vec3, kit: &mut ImpactKit) {
    let ImpactKit {
        assets,
        materials,
        ring,
        ground_ring,
        rng,
        commands,
    } = kit;
    let (tan_a, tan_b) = normal.any_orthonormal_pair();

    // --- Contact flash: one big additive bloom at the strike, gone in a frame or two.
    spawn_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.ping_material(),
            lifetime: look().splash_flash_lifetime,
            origin: position + normal * look().flash_clearance_m,
            drift: Vec3::ZERO,
            frames: 1,
            flipbook: Flipbook::Loop {
                start: 0.0,
                rate: 0.0,
            },
            start_size: look().splash_flash_size,
            end_size: look().splash_flash_size * look().splash_flash_end_scale,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: 0.0,
            erosion_end: 0.0,
            rotation: None,
        },
    );

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
    for _ in 0..count.min(look().ejecta_count.1) {
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
                material: assets.ejecta_material(),
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
                material: assets.dirt_material(look().plume_alpha),
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
    // and gone well before the fountain lands — the pressure wave, distinct from the haze below.
    spawn_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.shock_ring_material(),
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
            material: assets.dirt_material(look().splash_cloud_alpha),
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

    // --- Ground scar: one flat, ground-oriented gouge on the terrain — its OWN long-lived ring so it
    // isn't evicted within a frame by the sub-second billboard storm sharing BILLBOARD_CAP.
    spawn_billboard(
        commands,
        materials,
        &mut ground_ring.0,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.ground_mark_material(),
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

    use bevy::mesh::MeshTag;
    use bevy::render::storage::ShaderBuffer;

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
            .init_resource::<Assets<VfxBillboardMaterial>>()
            .init_resource::<Assets<ShaderBuffer>>()
            .init_resource::<BillboardFamilies>()
            .init_resource::<Time>()
            .insert_resource(ViewRng::seeded(42))
            .add_observer(spawn_impact_read);
        app.insert_resource(ImpactAssets::test_stub());
        app
    }

    /// Fire a terrain impact (the default surface) — keeps every existing terrain/MG test byte-for-
    /// byte, including the RNG draw order the small read pins.
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

    /// A non-sparking substance produces powder alone. A class-based metal recipe would add a
    /// flash and sparks here even though the resolved material explicitly emits neither.
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
            "one powder mass, no metal flash or spark fan"
        );
    }

    /// The ground scars ride their OWN eviction ring, bounded by GROUND_MARK_CAP — insulated from the
    /// shared billboard ring so a storm can't evict a fresh multi-second scar within a frame — and
    /// hold their own lane-table slots past the shared ring's, one per live scar.
    #[test]
    fn ground_scars_are_ring_capped() {
        let mut app = harness();
        for _ in 0..GROUND_MARK_CAP + 8 {
            trigger_impact(&mut app, Vec3::Y, LAYERED);
        }
        let ring = &app.world().resource::<GroundMarkRing>().0;
        assert_eq!(
            ring.live.len(),
            GROUND_MARK_CAP,
            "ground scars stay bounded by their own cap"
        );
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
