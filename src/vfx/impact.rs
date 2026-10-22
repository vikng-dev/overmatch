//! View-only impact dressing for [`Impact`].
//!
//! Invariant (ADR-0014): this module only reads simulation events and spawns transient render
//! entities. Caliber and `ImpactSurface` choose the visual family; no visual entity affects damage.
//! Product invariant (ADR-0023): visual scale and duration derive from physical event inputs, never
//! viewer distance.

use std::collections::VecDeque;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::ballistics::{Impact, ImpactOutcome, ImpactSurface, TRACER_MAX_CALIBER};
use crate::view::PlayerView;

use super::ViewRng;
use super::billboard::{
    BillboardRing, BillboardSpec, Flipbook, SOFT_PARTICLE_DEPTH, VfxBillboardMaterial, VfxParams,
    gradient_lut, spawn_ballistic_billboard, spawn_billboard, spawn_billboard_ring, unit_quad,
};

// Alpha blending lets dust darken and occlude rather than behave as an emissive effect.

/// Acceleration on everything an impact THROWS — dirt ejecta, the armor spark fan, the spall puff:
/// they leave the surface at launch speed and fall. Smoke, dust, haze and flashes are buoyant or
/// static and keep their straight drift.
const DEBRIS_GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

/// Dust billow lifetime range (s): a short kicked cloud, not a smoke column; at the MG's cyclic rate
/// anything longer stacks into fog. The RANGE is the per-impact variation.
const DUST_LIFETIME: (f32, f32) = (0.4, 0.6);
/// Dust billow size (m): birth diameter easing out to this by end of life (the cloud blooms fast).
const DUST_SIZE: (f32, f32) = (0.35, 1.5);
/// Dust drift: a push OUT along the surface normal plus a gentle rise (kicked debris lofts).
const DUST_NORMAL_PUSH: f32 = 1.1;
const DUST_RISE: f32 = 0.5;
/// Dust alpha multiplier — softer than the muzzle smoke; dust off a hit is thin.
const DUST_ALPHA: f32 = 0.6;
/// Faint heat on the young dust (the impact flash lights it for an instant) — kept low; dust is lit
/// mass, not an emitter.
const DUST_GLOW: f32 = 2.5;
/// Slow flipbook over the 4-frame dust atlas (frames/s) and a per-impact roll/spin bound (rad, rad/s).
const DUST_FRAME_RATE: f32 = 4.0;
const DUST_SPIN_MAX: f32 = 0.8;

// --- The additive ping: the 1-frame instant of contact (a small round glow at the hit point). Hot,
// premultiplied-additive, gone in a couple of frames — the flash the eye latches to.

/// Ping lifetime (s): 1–2 frames — the contact spark, gone before it can read as a lingering glow.
const PING_LIFETIME: f32 = 0.05;
/// Ping size (m): a small hit flash. It shrinks slightly over its brief life.
const PING_SIZE: f32 = 0.5;
/// Emissive boost on the ping LUT's heat lane — additive + bloom carry the read.
const PING_GLOW: f32 = 9.0;

// --- The spark layer (survey tricks 7/8): a few short velocity-elongated additive streaks kicked
// along/around the hit normal. Fewer/bigger/eroded beats many small — the counts stay tiny.

/// Sparks per impact, inclusive range (small — both MGs cycling is ~25 impacts/s).
const SPARK_COUNT: (u32, u32) = (2, 4);
/// Spark launch speed range (m/s) — brisk enough to visibly kick off the surface in ~0.15 s.
const SPARK_SPEED: (f32, f32) = (6.0, 14.0);
/// Spark lifetime range (s): a streak, not an ember shower.
const SPARK_LIFETIME: (f32, f32) = (0.10, 0.18);
/// Streak length as seconds-of-travel (length = speed × this): the velocity-elongation that reads
/// as motion blur (survey trick 8's "length ∝ speed").
const SPARK_STRETCH: (f32, f32) = (0.035, 0.055);
/// Streak width as a fraction of its length — needle-thin.
const SPARK_WIDTH_RATIO: f32 = 0.09;
/// Emissive boost on the spark LUT's heat lane (additive + bloom carry the read).
const SPARK_GLOW: f32 = 10.0;
/// Cone spread around the normal: tangent magnitude range before renormalizing (0 = straight out
/// along the normal, 1 ≈ 45°).
const SPARK_SPREAD: (f32, f32) = (0.15, 0.9);

// --- The 88 terrain SPLASH (caliber ≥ TRACER_MAX_CALIBER): a large-caliber AP round striking soil.
// Layered on the SAME billboard machinery (dirt-recolored LUTs, tall aspect for the plume, drift for
// the rise) — no new systems, no new pipelines (every layer is the already-warmed Add flash or Blend
// smoke pipeline). Physical scale from period gun-camera / range footage of big-AP soil strikes:
// dirt fountains in the ~4–10 m band with ~1.5–2 s hang. Chosen here: a ~7.7 m plume (5.5 m size ×
// 1.4 aspect) rising ~4.5 m/s over 1.5 s — mid-band, honest, NOT scaled to screen or range.
//
// The layers' lifetimes are staggered so they end one at a time — flash first, the lingering haze
// seconds after everything else.

/// Contact flash: a bright additive bloom at the instant of the strike (the round's kinetic flash +
/// dust ignition). Big and brief — size (m), lifetime (s). Maximal on frame one, shrinking after.
const SPLASH_FLASH_SIZE: f32 = 2.3;
const SPLASH_FLASH_LIFETIME: f32 = 0.04;

/// Ejecta streaks: the spark-streak geometry recolored to dirt (Blend, not hot), thrown in an
/// up-biased cone — clods and grit kicked off the strike. Count range, speed (m/s), lifetime (s),
/// and how strongly the launch cone is pulled toward straight up (0 = along the normal, 1 = up).
const EJECTA_COUNT: (u32, u32) = (8, 12);
const EJECTA_SPEED: (f32, f32) = (7.0, 18.0);
const EJECTA_LIFETIME: (f32, f32) = (0.19, 0.3);
const EJECTA_UP_BIAS: f32 = 0.55;
/// Ejecta streak stretch (seconds-of-travel → length) and width fraction — chunkier than the metal
/// sparks (soil clods, not needles).
const EJECTA_STRETCH: (f32, f32) = (0.03, 0.05);
const EJECTA_WIDTH_RATIO: f32 = 0.28;

/// Dirt plume: the tall central fountain — 1–2 blooming, rising, tall-aspect Blend billboards.
/// Count, size ease (m, base — aspect makes it tall), the tall aspect (y > x), rise speed (m/s), and
/// lifetime (s). Final height ≈ SPLASH_PLUME_SIZE.1 × aspect.y ≈ 7.7 m.
const SPLASH_PLUME_COUNT: (u32, u32) = (1, 2);
const SPLASH_PLUME_SIZE: (f32, f32) = (1.6, 5.5);
const SPLASH_PLUME_ASPECT: Vec3 = Vec3::new(0.7, 1.4, 1.0);
const SPLASH_PLUME_RISE: f32 = 4.5;
const SPLASH_PLUME_LIFETIME: f32 = 1.5;

/// Ground shock ring: the ground-lying billboard that races outward flat along the surface at the
/// strike instant — the pressure wave, not the dust that follows it. Size ease (m: birth diameter →
/// ~8 m across, most of it inside the first fifth of its life on the ease-out), lifetime (s), alpha,
/// and the erosion sharpness that gives the front a hard edge instead of a soft dust blur (the
/// dirt masses use [`ImpactAssets::dirt_material`]'s 2.2). The lingering haze below carries the slow
/// dust read, so this layer is free to be purely the wave.
const SPLASH_RING_SIZE: (f32, f32) = (1.2, 8.0);
const SPLASH_RING_LIFETIME: f32 = 0.6;
const SPLASH_RING_ALPHA: f32 = 0.4;
const SPLASH_RING_SHARPNESS: f32 = 6.0;

/// Lingering dust cloud: the large, low-alpha brown haze that hangs after the fountain falls — the
/// slowest layer, still thinning long after every other layer is gone. Size ease (m — it dissipates
/// by spreading, not by blinking out), lifetime (s), alpha, and its slow settle-height rise (m/s).
const SPLASH_CLOUD_SIZE: (f32, f32) = (3.0, 9.0);
const SPLASH_CLOUD_LIFETIME: f32 = 8.0;
const SPLASH_CLOUD_ALPHA: f32 = 0.35;
const SPLASH_CLOUD_RISE: f32 = 0.35;

/// Ground scar: one flat, ground-oriented disturbed-earth mark an 88 AP strike gouges. Footprint
/// (m, square), lifetime (s, the slowest-fading layer). Lives in its OWN ring ([`GroundMarkRing`],
/// cap [`GROUND_MARK_CAP`]) so a multi-second scar isn't evicted within a frame by the sub-second
/// billboard storm sharing [`BILLBOARD_CAP`].
const GROUND_MARK_SIZE: f32 = 2.0;
const GROUND_MARK_LIFETIME: f32 = 6.0;
/// The scar ring cap: a handful of recent gouges. Small — scars are rare (88-only) and long-lived.
const GROUND_MARK_CAP: usize = 16;

// --- The 88 on ARMOR (caliber ≥ TRACER_MAX_CALIBER, surface Armor): steel is categorically NOT dirt
// (War Thunder Drone-Age dev material + GHPC reference). A white-hot contact flash, a dense fast hot
// spark fan, and a small gray spall/smoke puff — NO plume, NO shock ring, NO ground scar, NO lingering
// brown cloud. A brief flame lick is added ONLY when the round bit into the steel
// (`Impact.outcome.verdict().entered()` — a defeated embed or a clean perforation, never a ricochet). Rides the SAME warmed Add (flash /
// spark / flame) and Blend (spall) pipelines as the terrain read — no new pipeline permutations.

/// Armor contact flash: bigger and hotter than the terrain splash flash ([`SPLASH_FLASH_SIZE`] 2.3)
/// — the welding-bright instant of steel-on-steel. Size (m), lifetime (s), emissive boost.
const ARMOR_FLASH_SIZE: f32 = 2.7;
const ARMOR_FLASH_LIFETIME: f32 = 0.045;
const ARMOR_FLASH_GLOW: f32 = 13.0;

/// Dense spark fan: many fast hot streaks off the steel (scaled up from the MG garnish's 2–4). Count
/// range, speed (m/s — brisker than terrain), lifetime (s), stretch (seconds-of-travel → length),
/// width fraction (needle-thin), emissive boost, and cone spread (tangent magnitude before
/// renormalizing; 0 = straight along the fan axis, ~1 ≈ 45°).
const ARMOR_SPARK_COUNT: (u32, u32) = (14, 20);
const ARMOR_SPARK_SPEED: (f32, f32) = (16.0, 34.0);
const ARMOR_SPARK_LIFETIME: (f32, f32) = (0.09, 0.165);
const ARMOR_SPARK_STRETCH: (f32, f32) = (0.03, 0.05);
const ARMOR_SPARK_WIDTH_RATIO: f32 = 0.08;
const ARMOR_SPARK_GLOW: f32 = 14.0;
const ARMOR_SPARK_SPREAD: (f32, f32) = (0.2, 1.1);
/// On a ricochet (`Impact.deflection` present) the fan axis leans from the surface normal toward the
/// deflected outgoing direction by this fraction (0 = symmetric off the normal, 1 = fully along the
/// bounce) — a bounce throws its sparks the way it kicked off.
const ARMOR_SPARK_DEFLECT_BIAS: f32 = 0.6;

/// Small gray spall/smoke puff: one short low-alpha gray mass (steel spall, NOT a dirt cloud). Size
/// ease (m — blooms to ~1.3 m), lifetime (s), alpha, and a gentle rise + normal push (m/s).
const SPALL_PUFF_SIZE: (f32, f32) = (0.4, 1.3);
const SPALL_PUFF_LIFETIME: (f32, f32) = (0.22, 0.38);
const SPALL_PUFF_ALPHA: f32 = 0.5;
const SPALL_PUFF_RISE: f32 = 0.6;

/// Flame lick (penetration only): one brief warm additive flame off the breach — the round's hot
/// metal igniting as it bites in. Size ease (m, birth → death: full width on frame one, collapsing
/// after), lifetime (s), emissive boost, and a rise (m/s).
const FLAME_LICK_SIZE: (f32, f32) = (1.5, 0.8);
const FLAME_LICK_LIFETIME: f32 = 0.12;
const FLAME_LICK_GLOW: f32 = 8.0;
const FLAME_LICK_RISE: f32 = 1.2;

/// Independent eviction ring for the long-lived ground scars (see [`GROUND_MARK_CAP`]). Insulated
/// from the shared [`BillboardRing`] so an MG storm filling that ring can't evict a fresh 88 scar.
#[derive(Resource, Default)]
pub(super) struct GroundMarkRing(pub VecDeque<Entity>);

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
    /// Earthy-brown palette for the 88 splash's dirt masses (plume/ring/cloud) and its dirt ejecta.
    dirt_lut: Handle<Image>,
    /// Darker scorched-earth palette for the flat ground scar.
    scar_lut: Handle<Image>,
    /// Neutral gray palette for the armor read's spall/smoke (steel spall, never brown dirt) — the
    /// MG armor billow swap and the 88 armor puff both use it.
    spall_lut: Handle<Image>,
    /// White-hot palette for the armor read's contact flash + dense spark fan (and the flame lick's
    /// hot core) — whiter/hotter than the terrain `spark_lut`, which cools to ember orange.
    hot_lut: Handle<Image>,
}

#[cfg(test)]
impl ImpactAssets {
    /// Every handle default: the tests spawn real billboards and clone real material assets, but no
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
            spall_lut: Handle::default(),
            hot_lut: Handle::default(),
        }
    }
}

impl ImpactAssets {
    /// The dust-billow material template: alpha-blend mass (glow.y = 0 — the occluding contract),
    /// its own earthy LUT, soft erosion edges. Its 2×2 atlas frame lanes are set on spawn.
    fn dust_material(&self) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                // Moderate sharpness for soft dissolve edges; DUST_ALPHA overall.
                fade: Vec4::new(0.0, 2.4, 0.0, DUST_ALPHA),
                glow: Vec4::new(DUST_GLOW, 0.0, SOFT_PARTICLE_DEPTH, 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: self.dust_lut.clone(),
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// The ping material template: additive glow (glow.y = 1 — premultiplied additive), single-frame
    /// full-image round glow, hard erosion so it snaps rather than ghosts.
    fn ping_material(&self) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 1.0, 1.0, 0.0),
                fade: Vec4::new(0.0, 3.0, 0.0, 1.0),
                glow: Vec4::new(PING_GLOW, 1.0, 0.0, 0.0),
            },
            atlas: self.ping_atlas.clone(),
            lut: self.spark_lut.clone(),
            alpha_mode: AlphaMode::Add,
        }
    }

    /// The spark material template: additive (hot metal never darkens), a 2×2 tapered-streak atlas
    /// (each spark picks a random frame), hard erosion edge so the streak dissolves crisply.
    pub(super) fn spark_material(&self) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, 3.0, 0.0, 1.0),
                glow: Vec4::new(SPARK_GLOW, 1.0, 0.0, 0.0),
            },
            atlas: self.spark_atlas.clone(),
            lut: self.spark_lut.clone(),
            alpha_mode: AlphaMode::Add,
        }
    }

    /// A dirt-mass material for the 88 splash's plume/ring/cloud: the dust atlas recolored through
    /// the earthy `dirt_lut`, alpha-blend occluding mass (glow.y = 0 — never additive), overall
    /// `alpha` per layer. Shares the already-warmed Blend billboard pipeline.
    fn dirt_material(&self, alpha: f32) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, 2.2, 0.0, alpha),
                glow: Vec4::new(0.0, 0.0, SOFT_PARTICLE_DEPTH, 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: self.dirt_lut.clone(),
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// The ground shock-ring material: the dirt mass at [`SPLASH_RING_ALPHA`] with the erosion lane
    /// sharpened ([`SPLASH_RING_SHARPNESS`]) so the racing front cuts a hard edge instead of the
    /// soft dust blur the plume and haze want.
    ///
    /// The soft-particle fade the other dirt masses carry is CLEARED here: the ring is a flat quad
    /// laid 0.1 m over the surface, so its whole area sits inside the fade band and the layer would
    /// scale itself out of existence (the same reason the ground scar never carries one — see
    /// [`SOFT_PARTICLE_DEPTH`]).
    fn shock_ring_material(&self) -> VfxBillboardMaterial {
        let mut material = self.dirt_material(SPLASH_RING_ALPHA);
        material.params.fade.y = SPLASH_RING_SHARPNESS;
        material.params.glow.z = 0.0;
        material
    }

    /// The dirt-ejecta material: the spark-streak geometry recolored to dirt (Blend, no glow — soil
    /// clods are lit mass, not hot metal). Same Blend pipeline as the dust masses.
    fn ejecta_material(&self) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, 3.0, 0.0, 1.0),
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
    fn ground_mark_material(&self) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(3.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, 1.6, 0.0, 0.85),
                glow: Vec4::new(0.0, 0.0, 0.0, 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: self.scar_lut.clone(),
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// A gray spall-smoke mass for the armor read's puff: the dust atlas recolored through the
    /// neutral `spall_lut` (never the earthy dirt LUT — steel throws gray spall/smoke, not dirt),
    /// alpha-blend occluding mass, overall `alpha`. Shares the already-warmed Blend pipeline.
    fn spall_material(&self, alpha: f32) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, 2.4, 0.0, alpha),
                glow: Vec4::new(0.0, 0.0, SOFT_PARTICLE_DEPTH, 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: self.spall_lut.clone(),
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// The armor contact flash: a white-hot additive bloom, hotter/whiter than the terrain splash's
    /// warm flash ([`ImpactAssets::ping_material`] on the spark LUT) — welding-bright steel contact.
    /// The round-glow atlas through the white-hot `hot_lut`.
    fn armor_flash_material(&self) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 1.0, 1.0, 0.0),
                fade: Vec4::new(0.0, 3.0, 0.0, 1.0),
                glow: Vec4::new(ARMOR_FLASH_GLOW, 1.0, 0.0, 0.0),
            },
            atlas: self.ping_atlas.clone(),
            lut: self.hot_lut.clone(),
            alpha_mode: AlphaMode::Add,
        }
    }

    /// The armor spark material: the spark-streak atlas through the white-hot `hot_lut` — the dense
    /// fan's hotter, whiter streak (the terrain `spark_material` cools to ember orange). Additive.
    fn armor_spark_material(&self) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, 3.0, 0.0, 1.0),
                glow: Vec4::new(ARMOR_SPARK_GLOW, 1.0, 0.0, 0.0),
            },
            atlas: self.spark_atlas.clone(),
            lut: self.hot_lut.clone(),
            alpha_mode: AlphaMode::Add,
        }
    }

    /// The flame-lick material (penetration only): a brief warm additive flame off the breach — the
    /// spark LUT's orange tail read as fire, over the soft dust atlas so it reads as a lick, not a
    /// spark. Additive so it blooms; kept small and short (see the flame-lick constants).
    fn flame_material(&self) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, 2.0, 0.0, 1.0),
                glow: Vec4::new(FLAME_LICK_GLOW, 1.0, 0.0, 0.0),
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
        let lum = 0.10 + 0.42 * x;
        let earth = (1.0 - y) * 0.22;
        let color = LinearRgba::rgb(
            lum * (0.94 + earth),
            lum * (0.82 + earth * 0.55),
            lum * (0.64 + earth * 0.15),
        );
        let heat = 0.12 * x * (-y * 8.0).exp();
        (color, heat)
    });
    // Spark LUT: white-yellow hot at birth cooling toward ember orange, heat front-loaded so young
    // sparks bloom and old ones die as dim embers. Shared by the ping (a hot round glow).
    let spark_lut = gradient_lut(&mut images, |x, y| {
        let cool = 1.0 - y;
        let color = LinearRgba::rgb(x, x * (0.55 + 0.35 * cool), x * x * (0.12 + 0.5 * cool));
        (color, x * (0.35 + 0.65 * cool))
    });
    // Dirt LUT for the 88 splash masses: an earthy brown, brighter/tanner where the sprite signal is
    // strong (sunlit dirt face) sinking to a dark damp brown in shadow; darkening slightly as the
    // cloud ages (Y). No heat — kicked soil is inert lit mass, never an emitter.
    let dirt_lut = gradient_lut(&mut images, |x, y| {
        let lum = 0.06 + 0.34 * x;
        let age = 1.0 - y;
        let color = LinearRgba::rgb(
            lum * (0.78 + 0.30 * age),
            lum * (0.58 + 0.20 * age),
            lum * (0.40 + 0.12 * age),
        );
        (color, 0.0)
    });
    // Scar LUT: the same earth pulled much darker — a scorched, disturbed-earth gouge that reads as a
    // stain on the terrain, not a bright decal. No heat.
    let scar_lut = gradient_lut(&mut images, |x, _| {
        let lum = 0.03 + 0.16 * x;
        let color = LinearRgba::rgb(lum * 0.85, lum * 0.62, lum * 0.45);
        (color, 0.0)
    });
    // Spall LUT for the armor read's gray spall/smoke: a NEUTRAL (faintly cool) gray with NO earth
    // tint — steel throws gray spall and smoke, never brown dirt. Darkens slightly with age (Y); a
    // whisper of heat only in the brightest young texels (the strike lights it for an instant).
    let spall_lut = gradient_lut(&mut images, |x, y| {
        let lum = 0.10 + 0.40 * x;
        let color = LinearRgba::rgb(lum * 0.88, lum * 0.90, lum * 0.94);
        let heat = 0.10 * x * (-y * 8.0).exp();
        (color, heat)
    });
    // Hot LUT for the armor flash + spark fan: white-hot at birth (all channels high, a touch of blue
    // in the hottest core) cooling toward white-yellow — hotter and WHITER than the spark LUT's orange
    // ember tail (armor contact is a welding-bright flash, not a campfire ember). Heat front-loaded.
    let hot_lut = gradient_lut(&mut images, |x, y| {
        let cool = 1.0 - y;
        let color = LinearRgba::rgb(x, x * (0.85 + 0.12 * cool), x * (0.7 + 0.2 * cool));
        (color, x * (0.7 + 0.6 * cool))
    });
    commands.insert_resource(ImpactAssets {
        quad: unit_quad(&mut meshes),
        dust_atlas: asset_server.load("vfx/impact_dust.png"),
        dust_lut,
        // A generic round glow (the same `light_01`-derived sprite the MG core uses; the asset
        // server dedupes the load and shares the GPU texture).
        ping_atlas: asset_server.load("vfx/mg_core.png"),
        spark_atlas: asset_server.load("vfx/spark_atlas.png"),
        spark_lut,
        dirt_lut,
        scar_lut,
        spall_lut,
        hot_lut,
    });
}

/// What every impact read spawns into: its assets, the billboard material store, the shared
/// billboard and ground-mark rings, the view RNG and `Commands`.
#[derive(SystemParam)]
pub(super) struct ImpactKit<'w, 's> {
    assets: Res<'w, ImpactAssets>,
    materials: ResMut<'w, Assets<VfxBillboardMaterial>>,
    ring: ResMut<'w, BillboardRing>,
    ground_ring: ResMut<'w, GroundMarkRing>,
    rng: ResMut<'w, ViewRng>,
    pub(super) commands: Commands<'w, 's>,
}

/// Drop the layered read at each shell impact — branching on BOTH the round's physical caliber and
/// the `surface` it struck. At/above [`TRACER_MAX_CALIBER`] the 88 lands either the big terrain
/// splash ([`spawn_terrain_read`]) or the armor read ([`spawn_big_armor`]); below it, the MG's compact
/// dust-ping-spark read ([`spawn_small_impact`], which recolors its billow gray on armor but keeps
/// the terrain path byte-for-byte). `normal`/`to_camera` are resolved once, before any path draws
/// RNG, so the small path's RNG sequence is unchanged and the surface pick costs no RNG.
fn spawn_impact_read(
    impact: On<Impact>,
    mut kit: ImpactKit,
    camera: Query<&GlobalTransform, With<PlayerView>>,
) {
    let normal = impact.normal.try_normalize().unwrap_or(Vec3::Y);
    let to_camera = camera
        .single()
        .map(|cam| cam.translation() - impact.position)
        .unwrap_or(Vec3::Z);
    match impact.surface {
        ImpactSurface::Terrain => {
            spawn_terrain_read(impact.position, normal, to_camera, impact.caliber, &mut kit)
        }
        ImpactSurface::Armor => {
            if impact.caliber >= TRACER_MAX_CALIBER {
                spawn_big_armor(impact.position, normal, to_camera, impact.outcome, &mut kit);
            } else {
                spawn_small_impact(
                    impact.position,
                    normal,
                    to_camera,
                    ImpactSurface::Armor,
                    &mut kit,
                );
            }
        }
    }
}

/// The TERRAIN half of the read above, as a callable: the 88's big splash at/above
/// [`TRACER_MAX_CALIBER`], the MG's compact dust-ping-spark read below it. `normal` must be unit;
/// `to_camera` need not be. Shared with [`super::scatter_hit`], whose crossings are terrain-class
/// surfaces and so take this exact dispatch rather than a read of their own.
pub(super) fn spawn_terrain_read(
    position: Vec3,
    normal: Vec3,
    to_camera: Vec3,
    caliber: f32,
    kit: &mut ImpactKit,
) {
    if caliber >= TRACER_MAX_CALIBER {
        spawn_big_splash(position, normal, to_camera, kit);
    } else {
        spawn_small_impact(position, normal, to_camera, ImpactSurface::Terrain, kit);
    }
}

/// The MG / small-caliber read (the original terrain body, plus a gray-spall LUT swap on armor):
/// dust billow (mass) + ping (contact flash) + a few sparks (crisp garnish), all on the shared
/// billboard ring. This is what makes the four invisible non-tracer rounds of the belt cycle register
/// at the target. On ARMOR the billow's LUT swaps from the earthy dust palette to the neutral gray
/// `spall_lut` (the honest minimum recolor — a small round on steel throws gray spall, not dirt); the
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
        if surface == ImpactSurface::Armor {
            m.lut = assets.spall_lut.clone();
        }
        m
    };

    // --- Dust billow: one alpha-blended eroding billboard, random frame/roll/spin, blooming and
    // drifting out along the surface normal (with a gentle rise).
    let dust_size = rng.range(DUST_SIZE.0, DUST_SIZE.1);
    spawn_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: billow,
            lifetime: rng.range(DUST_LIFETIME.0, DUST_LIFETIME.1),
            origin: position + normal * (DUST_SIZE.0 * 0.5),
            drift: normal * DUST_NORMAL_PUSH + Vec3::Y * DUST_RISE,
            frames: 4,
            flipbook: Flipbook::Loop {
                start: rng.range(0.0, 4.0),
                rate: DUST_FRAME_RATE,
            },
            start_size: DUST_SIZE.0,
            end_size: dust_size,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: rng.range(-DUST_SPIN_MAX, DUST_SPIN_MAX),
            erosion_end: 1.0,
            rotation: None,
        },
    );

    // --- Ping: one small additive round glow at the hit point, 1–2 frames — the instant of contact.
    spawn_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.ping_material(),
            lifetime: PING_LIFETIME,
            origin: position + normal * 0.05,
            drift: Vec3::ZERO,
            frames: 1,
            flipbook: Flipbook::Loop {
                start: 0.0,
                rate: 0.0,
            },
            start_size: PING_SIZE,
            end_size: PING_SIZE * 0.7,
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
    let count =
        SPARK_COUNT.0 + (rng.next_f32() * (SPARK_COUNT.1 - SPARK_COUNT.0 + 1) as f32) as u32;
    for _ in 0..count.min(SPARK_COUNT.1) {
        let theta = rng.range(0.0, std::f32::consts::TAU);
        let spread = rng.range(SPARK_SPREAD.0, SPARK_SPREAD.1);
        let dir = (normal + (tan_a * theta.cos() + tan_b * theta.sin()) * spread).normalize();
        let speed = rng.range(SPARK_SPEED.0, SPARK_SPEED.1);
        let length = speed * rng.range(SPARK_STRETCH.0, SPARK_STRETCH.1);
        spawn_billboard(
            commands,
            materials,
            ring,
            assets.quad.clone(),
            BillboardSpec {
                material: assets.spark_material(),
                lifetime: rng.range(SPARK_LIFETIME.0, SPARK_LIFETIME.1),
                origin: position + dir * (length * 0.5),
                drift: dir * speed,
                frames: 4,
                flipbook: Flipbook::Loop {
                    start: rng.range(0.0, 4.0),
                    rate: 0.0,
                },
                start_size: length,
                end_size: length * 0.6,
                aspect: Vec3::new(SPARK_WIDTH_RATIO, 1.0, 1.0),
                roll: 0.0,
                spin: 0.0,
                erosion_end: 1.0,
                rotation: Some(spark_orientation(dir, to_camera)),
            },
        );
    }
}

/// The 88 terrain SPLASH (caliber ≥ TRACER_MAX_CALIBER): the layered large-caliber soil-strike read.
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
            lifetime: SPLASH_FLASH_LIFETIME,
            origin: position + normal * 0.1,
            drift: Vec3::ZERO,
            frames: 1,
            flipbook: Flipbook::Loop {
                start: 0.0,
                rate: 0.0,
            },
            start_size: SPLASH_FLASH_SIZE,
            end_size: SPLASH_FLASH_SIZE * 0.8,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: 0.0,
            erosion_end: 0.0,
            rotation: None,
        },
    );

    // --- Ejecta streaks: soil clods thrown in an up-biased cone (blend the normal toward straight up
    // by EJECTA_UP_BIAS, then spread), each a dirt-recolored stretched streak launched at speed and
    // falling under DEBRIS_GRAVITY. The streak's ORIENTATION is baked at spawn from the launch
    // direction, so it holds the ballistic-initial heading for its ~0.3 s life.
    let up_axis = (normal * (1.0 - EJECTA_UP_BIAS) + Vec3::Y * EJECTA_UP_BIAS)
        .try_normalize()
        .unwrap_or(Vec3::Y);
    let count =
        EJECTA_COUNT.0 + (rng.next_f32() * (EJECTA_COUNT.1 - EJECTA_COUNT.0 + 1) as f32) as u32;
    for _ in 0..count.min(EJECTA_COUNT.1) {
        let theta = rng.range(0.0, std::f32::consts::TAU);
        let spread = rng.range(SPARK_SPREAD.0, SPARK_SPREAD.1);
        let dir = (up_axis + (tan_a * theta.cos() + tan_b * theta.sin()) * spread).normalize();
        let speed = rng.range(EJECTA_SPEED.0, EJECTA_SPEED.1);
        let length = speed * rng.range(EJECTA_STRETCH.0, EJECTA_STRETCH.1);
        spawn_ballistic_billboard(
            commands,
            materials,
            ring,
            assets.quad.clone(),
            BillboardSpec {
                material: assets.ejecta_material(),
                lifetime: rng.range(EJECTA_LIFETIME.0, EJECTA_LIFETIME.1),
                origin: position + dir * (length * 0.5),
                drift: dir * speed,
                frames: 4,
                flipbook: Flipbook::Loop {
                    start: rng.range(0.0, 4.0),
                    rate: 0.0,
                },
                start_size: length,
                end_size: length * 0.7,
                aspect: Vec3::new(EJECTA_WIDTH_RATIO, 1.0, 1.0),
                roll: 0.0,
                spin: 0.0,
                erosion_end: 1.0,
                rotation: Some(spark_orientation(dir, to_camera)),
            },
            DEBRIS_GRAVITY,
        );
    }

    // --- Dirt plume: 1–2 tall camera-facing dirt columns rising off the strike, blooming to full
    // height over their life then eroding out.
    let plumes = SPLASH_PLUME_COUNT.0
        + (rng.next_f32() * (SPLASH_PLUME_COUNT.1 - SPLASH_PLUME_COUNT.0 + 1) as f32) as u32;
    for _ in 0..plumes.min(SPLASH_PLUME_COUNT.1) {
        spawn_billboard(
            commands,
            materials,
            ring,
            assets.quad.clone(),
            BillboardSpec {
                material: assets.dirt_material(0.7),
                lifetime: SPLASH_PLUME_LIFETIME,
                origin: position + normal * (SPLASH_PLUME_SIZE.0 * 0.5),
                drift: Vec3::Y * SPLASH_PLUME_RISE,
                frames: 4,
                flipbook: Flipbook::Loop {
                    start: rng.range(0.0, 4.0),
                    rate: 0.0,
                },
                start_size: SPLASH_PLUME_SIZE.0,
                end_size: SPLASH_PLUME_SIZE.1,
                aspect: SPLASH_PLUME_ASPECT,
                roll: rng.range(-0.2, 0.2),
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
            lifetime: SPLASH_RING_LIFETIME,
            origin: position + normal * 0.1,
            drift: Vec3::ZERO,
            frames: 4,
            flipbook: Flipbook::Loop {
                start: rng.range(0.0, 4.0),
                rate: 0.0,
            },
            start_size: SPLASH_RING_SIZE.0,
            end_size: SPLASH_RING_SIZE.1,
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
            material: assets.dirt_material(SPLASH_CLOUD_ALPHA),
            lifetime: SPLASH_CLOUD_LIFETIME,
            origin: position + normal * (SPLASH_CLOUD_SIZE.0 * 0.5) + Vec3::Y * 0.5,
            drift: Vec3::Y * SPLASH_CLOUD_RISE,
            frames: 4,
            flipbook: Flipbook::Loop {
                start: rng.range(0.0, 4.0),
                rate: 0.0,
            },
            start_size: SPLASH_CLOUD_SIZE.0,
            end_size: SPLASH_CLOUD_SIZE.1,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: 0.0,
            erosion_end: 1.0,
            rotation: None,
        },
    );

    // --- Ground scar: one flat, ground-oriented gouge on the terrain — its OWN long-lived ring so it
    // isn't evicted within a frame by the sub-second billboard storm sharing BILLBOARD_CAP.
    spawn_billboard_ring(
        commands,
        materials,
        &mut ground_ring.0,
        GROUND_MARK_CAP,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.ground_mark_material(),
            lifetime: GROUND_MARK_LIFETIME,
            origin: position + normal * 0.05,
            drift: Vec3::ZERO,
            frames: 4,
            flipbook: Flipbook::Loop {
                start: 3.0,
                rate: 0.0,
            },
            start_size: GROUND_MARK_SIZE,
            end_size: GROUND_MARK_SIZE * 1.1,
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

/// The 88 ARMOR read (caliber ≥ TRACER_MAX_CALIBER, surface Armor): the spark-on-steel read — a
/// white-hot contact flash, a dense fast hot spark fan, and a small gray spall/smoke puff, all on the
/// shared ring. NO plume, NO shock ring, NO lingering cloud, and — deliberately — NO ground scar
/// (steel isn't gouged like soil; the armor read touches only the shared billboard ring). A brief
/// flame lick is appended ONLY when `verdict.entered()` (the round bit into the steel). When `deflection` is
/// present (a ricochet) the spark fan leans toward the bounce direction; otherwise it splays off the
/// surface normal. Physical read (module doc), not screen-relative.
fn spawn_big_armor(
    position: Vec3,
    normal: Vec3,
    to_camera: Vec3,
    outcome: ImpactOutcome,
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
    let axis = match outcome.deflection().and_then(|d| d.try_normalize()) {
        Some(defl) => (normal * (1.0 - ARMOR_SPARK_DEFLECT_BIAS) + defl * ARMOR_SPARK_DEFLECT_BIAS)
            .try_normalize()
            .unwrap_or(normal),
        None => normal,
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
            lifetime: ARMOR_FLASH_LIFETIME,
            origin: position + normal * 0.1,
            drift: Vec3::ZERO,
            frames: 1,
            flipbook: Flipbook::Loop {
                start: 0.0,
                rate: 0.0,
            },
            start_size: ARMOR_FLASH_SIZE,
            end_size: ARMOR_FLASH_SIZE * 0.8,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: 0.0,
            erosion_end: 0.0,
            rotation: None,
        },
    );

    // --- Dense hot spark fan: many fast white-hot streaks in a cone around the fan axis, each a
    // fixed-orientation billboard elongated along its own flight direction and falling under
    // DEBRIS_GRAVITY (the orientation is baked at spawn, so a streak holds its launch heading).
    let count = ARMOR_SPARK_COUNT.0
        + (rng.next_f32() * (ARMOR_SPARK_COUNT.1 - ARMOR_SPARK_COUNT.0 + 1) as f32) as u32;
    for _ in 0..count.min(ARMOR_SPARK_COUNT.1) {
        let theta = rng.range(0.0, std::f32::consts::TAU);
        let spread = rng.range(ARMOR_SPARK_SPREAD.0, ARMOR_SPARK_SPREAD.1);
        let dir = (axis + (tan_a * theta.cos() + tan_b * theta.sin()) * spread).normalize();
        let speed = rng.range(ARMOR_SPARK_SPEED.0, ARMOR_SPARK_SPEED.1);
        let length = speed * rng.range(ARMOR_SPARK_STRETCH.0, ARMOR_SPARK_STRETCH.1);
        spawn_ballistic_billboard(
            commands,
            materials,
            ring,
            assets.quad.clone(),
            BillboardSpec {
                material: assets.armor_spark_material(),
                lifetime: rng.range(ARMOR_SPARK_LIFETIME.0, ARMOR_SPARK_LIFETIME.1),
                origin: position + dir * (length * 0.5),
                drift: dir * speed,
                frames: 4,
                flipbook: Flipbook::Loop {
                    start: rng.range(0.0, 4.0),
                    rate: 0.0,
                },
                start_size: length,
                end_size: length * 0.6,
                aspect: Vec3::new(ARMOR_SPARK_WIDTH_RATIO, 1.0, 1.0),
                roll: 0.0,
                spin: 0.0,
                erosion_end: 1.0,
                rotation: Some(spark_orientation(dir, to_camera)),
            },
            DEBRIS_GRAVITY,
        );
    }

    // --- Small gray spall puff: one short gray smoke mass off the strike (steel spall, never dirt),
    // pushed gently out along the normal, rising, then settling down the plate under DEBRIS_GRAVITY.
    let puff_size = rng.range(SPALL_PUFF_SIZE.0, SPALL_PUFF_SIZE.1);
    spawn_ballistic_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.spall_material(SPALL_PUFF_ALPHA),
            lifetime: rng.range(SPALL_PUFF_LIFETIME.0, SPALL_PUFF_LIFETIME.1),
            origin: position + normal * (SPALL_PUFF_SIZE.0 * 0.5),
            drift: normal * 0.3 + Vec3::Y * SPALL_PUFF_RISE,
            frames: 4,
            flipbook: Flipbook::Loop {
                start: rng.range(0.0, 4.0),
                rate: 0.0,
            },
            start_size: SPALL_PUFF_SIZE.0,
            end_size: puff_size,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: rng.range(-DUST_SPIN_MAX, DUST_SPIN_MAX),
            erosion_end: 1.0,
            rotation: None,
        },
        DEBRIS_GRAVITY,
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
                lifetime: FLAME_LICK_LIFETIME,
                origin: position + normal * (FLAME_LICK_SIZE.1 * 0.5),
                drift: normal * 0.5 + Vec3::Y * FLAME_LICK_RISE,
                frames: 4,
                flipbook: Flipbook::Loop {
                    start: rng.range(0.0, 4.0),
                    rate: 0.0,
                },
                start_size: FLAME_LICK_SIZE.0,
                end_size: FLAME_LICK_SIZE.1,
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
    // The camera direction's component perpendicular to the streak axis; parallel/degenerate views
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

    use crate::vfx::billboard::{BILLBOARD_CAP, Billboard};

    /// An MG-belt round: below TRACER_MAX_CALIBER, so the small dust-ping-spark read.
    const MG_CALIBER: f32 = 0.0079;
    /// The 88's AP round: at/above TRACER_MAX_CALIBER, so the big terrain splash.
    const BIG_CALIBER: f32 = 0.088;

    /// Minimal app carrying what the impact observer reads. Real `Assets` stores (initialized bare,
    /// no asset plugins) so the per-billboard material clones run for real; fixed-seed view RNG; no
    /// camera (spark facing falls back).
    fn harness() -> App {
        let mut app = App::new();
        app.init_resource::<BillboardRing>()
            .init_resource::<GroundMarkRing>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<VfxBillboardMaterial>>()
            .init_resource::<Time>()
            .insert_resource(ViewRng::seeded(42))
            .add_observer(spawn_impact_read);
        app.insert_resource(ImpactAssets::test_stub());
        app
    }

    /// Fire a terrain impact (the default surface) — keeps every existing terrain/MG test byte-for-
    /// byte, including the RNG draw order the small read pins.
    fn trigger_impact(app: &mut App, normal: Vec3, caliber: f32) {
        trigger_surface(
            app,
            normal,
            caliber,
            ImpactSurface::Terrain,
            ImpactOutcome::Struck,
        );
    }

    /// Fire an impact on an explicit surface (with verdict + deflection) — the armor tests.
    fn trigger_surface(
        app: &mut App,
        normal: Vec3,
        caliber: f32,
        surface: ImpactSurface,
        outcome: ImpactOutcome,
    ) {
        app.world_mut().trigger(Impact {
            position: Vec3::ZERO,
            normal,
            caliber,
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

    /// Sparks are the needle-thin fixed-orientation streaks; dust + ping are the aspect-1 camera
    /// facers. This splits them for the assertions below.
    fn sparks(app: &mut App) -> Vec<(Vec3, f32)> {
        let world = app.world_mut();
        let mut q = world.query::<(&Billboard, &Transform)>();
        q.iter(world)
            .filter(|(b, _)| b.aspect.x < 0.2)
            .map(|(b, t)| {
                (
                    b.drift,
                    t.rotation.mul_vec3(Vec3::Y).dot(b.drift.normalize()),
                )
            })
            .collect()
    }

    /// The ground scars ride their OWN eviction ring, bounded by GROUND_MARK_CAP — insulated from the
    /// shared billboard ring so a storm can't evict a fresh multi-second scar within a frame.
    #[test]
    fn ground_scars_are_ring_capped() {
        let mut app = harness();
        for _ in 0..GROUND_MARK_CAP + 8 {
            trigger_impact(&mut app, Vec3::Y, BIG_CALIBER);
        }
        assert_eq!(
            app.world().resource::<GroundMarkRing>().0.len(),
            GROUND_MARK_CAP,
            "ground scars stay bounded by their own cap"
        );
    }

    /// A degenerate (zero) normal still throws the read — sparks straight up, the terrain fallback —
    /// so the effect can never panic or vanish on a weird raycast.
    #[test]
    fn degenerate_normal_falls_back_up() {
        let mut app = harness();
        trigger_impact(&mut app, Vec3::ZERO, MG_CALIBER);
        let sparks = sparks(&mut app);
        assert!(sparks.len() >= SPARK_COUNT.0 as usize);
        for (drift, _) in sparks {
            assert!(drift.y > 0.0, "fallback sparks kick upward");
        }
    }

    /// The whole read rides the shared billboard ring: an impact storm is bounded by its cap, not
    /// unbounded entity growth (the same leak bound every other vfx layer pins).
    #[test]
    fn impact_storm_is_ring_capped() {
        let mut app = harness();
        for _ in 0..200 {
            trigger_impact(&mut app, Vec3::Y, MG_CALIBER);
        }
        let ring_len = app.world().resource::<BillboardRing>().0.len();
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
            BIG_CALIBER,
            ImpactSurface::Armor,
            ImpactOutcome::Struck,
        );
        let n0 = billboards(&mut no_pen);

        let mut pen = harness();
        trigger_surface(
            &mut pen,
            Vec3::Y,
            BIG_CALIBER,
            ImpactSurface::Armor,
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
