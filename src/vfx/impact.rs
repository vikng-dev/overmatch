//! View-layer impact VFX, layered per the survey's impact recipe (trick 7, scaled to what `Impact`
//! carries): a HYBRID read on every landed round —
//!   * a Kenney dust BILLOW (alpha-blend billboard, its own warm-gray→earth LUT) that expands and
//!     drifts along the surface normal — the mass of the read, replacing the old emissive sphere
//!     that read as a glowing circle rather than kicked dust;
//!   * a 1-frame additive PING (a small round glow) at the hit point — the instant of contact; and
//!   * a handful of stretched-billboard SPARKS (survey trick 8: velocity-elongated additive streaks,
//!     free motion blur) kicked in a cone around the hit's surface normal — the crisp hot garnish.
//!
//! All three ride the shared billboard machinery ([`super::billboard`]) — one aging system, one
//! eviction ring — so an impact storm stays bounded with the muzzle dressing. The read exists so the
//! four non-tracer rounds of an MG belt cycle still register at the target: the rounds themselves
//! stay invisible in flight (only every fifth gets a streak — `ballistics::on_fire_shell`), but
//! every round that lands billows + pings + sparks, so a burst visibly walks across whatever it is
//! hitting instead of one lone tracer arriving out of nowhere.
//!
//! The read now branches on the round's physical scale: `Impact` carries `caliber`, and at/above
//! [`TRACER_MAX_CALIBER`] (the same big/small boundary the muzzle + tracer paths use) the 88 lands
//! a full terrain SPLASH stack — contact flash, dirt ejecta, a tall dirt plume, a low dust ring, a
//! lingering cloud, and a flat ground scar — instead of the MG's compact dust-ping-spark read. The
//! scale is HONEST: the plume height/hang come from real large-caliber soil-strike footage, never
//! from screen size or camera distance (owner doctrine 2026-07-12 — no fake viewer assistance). The
//! MG path below [`TRACER_MAX_CALIBER`] is byte-for-byte the original small read. Armor-vs-terrain
//! surface differentiation (spark-on-steel vs dirt) is still deferred.
//!
//! Strictly view-only (ADR-0014): subscribes to the sim's [`Impact`] event and spawns short-lived
//! render entities that no sim system ever reads — safe on a predicting net client (the replica
//! still flies cosmetic shells and sparks `Impact`s; damage authority is untouched). Mounted by both
//! windowed client compositions (SP `ClientPlugin` and `NetClientPlugin`); the headless server and
//! the scripted harness never mount it.

use std::collections::VecDeque;

use bevy::prelude::*;

use crate::ballistics::{Impact, TRACER_MAX_CALIBER};

use super::ViewRng;
use super::billboard::{
    BillboardRing, BillboardSpec, VfxBillboardMaterial, VfxParams, gradient_lut, spawn_billboard,
    spawn_billboard_ring, unit_quad,
};

// --- The dust billow (survey trick 7): the mass of the impact read. Alpha-blend so it darkens and
// occludes like real kicked dust, eroding as it thins — never the old uniform-fading emissive ball.

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
// 1.4 aspect) rising ~4.5 m/s over 1.8 s — mid-band, honest, NOT scaled to screen or range.

/// Contact flash: a bright additive bloom at the instant of the strike (the round's kinetic flash +
/// dust ignition). Big and brief — size (m), lifetime (s).
const SPLASH_FLASH_SIZE: f32 = 2.3;
const SPLASH_FLASH_LIFETIME: f32 = 0.05;

/// Ejecta streaks: the spark-streak geometry recolored to dirt (Blend, not hot), thrown in an
/// up-biased cone — clods and grit kicked off the strike. Count range, speed (m/s), lifetime (s),
/// and how strongly the launch cone is pulled toward straight up (0 = along the normal, 1 = up).
const EJECTA_COUNT: (u32, u32) = (8, 12);
const EJECTA_SPEED: (f32, f32) = (7.0, 18.0);
const EJECTA_LIFETIME: (f32, f32) = (0.25, 0.4);
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
const SPLASH_PLUME_LIFETIME: f32 = 1.8;

/// Low dust ring: a wide, ground-hugging billboard that blooms outward along the surface — the dust
/// skirt thrown flat off the strike. Size ease (m), lifetime (s), low alpha.
const SPLASH_RING_SIZE: (f32, f32) = (1.0, 5.0);
const SPLASH_RING_LIFETIME: f32 = 0.8;
const SPLASH_RING_ALPHA: f32 = 0.4;

/// Lingering dust cloud: the large, low-alpha brown haze that hangs after the fountain falls — the
/// slowest layer. Size ease (m), lifetime (s), alpha.
const SPLASH_CLOUD_SIZE: (f32, f32) = (3.0, 6.0);
const SPLASH_CLOUD_LIFETIME: f32 = 3.0;
const SPLASH_CLOUD_ALPHA: f32 = 0.35;

/// Ground scar: one flat, ground-oriented disturbed-earth mark an 88 AP strike gouges. Footprint
/// (m, square), lifetime (s, the slowest-fading layer). Lives in its OWN ring ([`GroundMarkRing`],
/// cap [`GROUND_MARK_CAP`]) so a multi-second scar isn't evicted within a frame by the sub-second
/// billboard storm sharing [`BILLBOARD_CAP`].
const GROUND_MARK_SIZE: f32 = 2.0;
const GROUND_MARK_LIFETIME: f32 = 6.0;
/// The scar ring cap: a handful of recent gouges. Small — scars are rare (88-only) and long-lived.
const GROUND_MARK_CAP: usize = 16;

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
                glow: Vec4::new(DUST_GLOW, 0.0, 0.0, 0.0),
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
                glow: Vec4::new(0.0, 0.0, 0.0, 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: self.dirt_lut.clone(),
            alpha_mode: AlphaMode::Blend,
        }
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
    });
}

/// Drop the layered read at each shell impact — branching on the round's physical caliber. At/above
/// [`TRACER_MAX_CALIBER`] the 88 lands the big terrain splash ([`spawn_big_splash`]); below it, the
/// MG's compact dust-ping-spark read ([`spawn_small_impact`], byte-for-byte the original). `normal`
/// is resolved once, before either path draws RNG, so the small path's RNG sequence is unchanged.
fn spawn_impact_read(
    impact: On<Impact>,
    assets: Res<ImpactAssets>,
    mut materials: ResMut<Assets<VfxBillboardMaterial>>,
    mut ring: ResMut<BillboardRing>,
    mut ground_ring: ResMut<GroundMarkRing>,
    mut rng: ResMut<ViewRng>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut commands: Commands,
) {
    let normal = impact.normal.try_normalize().unwrap_or(Vec3::Y);
    let to_camera = camera
        .single()
        .map(|cam| cam.translation() - impact.position)
        .unwrap_or(Vec3::Z);
    if impact.caliber >= TRACER_MAX_CALIBER {
        spawn_big_splash(
            impact.position,
            normal,
            to_camera,
            &assets,
            &mut materials,
            &mut ring,
            &mut ground_ring,
            &mut rng,
            &mut commands,
        );
    } else {
        spawn_small_impact(
            impact.position,
            normal,
            to_camera,
            &assets,
            &mut materials,
            &mut ring,
            &mut rng,
            &mut commands,
        );
    }
}

/// The MG / small-caliber read (byte-for-byte the original `spawn_impact_read` body): dust billow
/// (mass) + ping (contact flash) + a few sparks (crisp garnish), all on the shared billboard ring.
/// This is what makes the four invisible non-tracer rounds of the belt cycle register at the target.
#[allow(clippy::too_many_arguments)]
fn spawn_small_impact(
    position: Vec3,
    normal: Vec3,
    to_camera: Vec3,
    assets: &ImpactAssets,
    materials: &mut Assets<VfxBillboardMaterial>,
    ring: &mut BillboardRing,
    rng: &mut ViewRng,
    commands: &mut Commands,
) {
    // --- Dust billow: one alpha-blended eroding billboard, random frame/roll/spin, blooming and
    // drifting out along the surface normal (with a gentle rise).
    let dust_size = rng.range(DUST_SIZE.0, DUST_SIZE.1);
    spawn_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.dust_material(),
            lifetime: rng.range(DUST_LIFETIME.0, DUST_LIFETIME.1),
            origin: position + normal * (DUST_SIZE.0 * 0.5),
            drift: normal * DUST_NORMAL_PUSH + Vec3::Y * DUST_RISE,
            frames: 4,
            start_frame: rng.range(0.0, 4.0),
            frame_rate: DUST_FRAME_RATE,
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
            start_frame: 0.0,
            frame_rate: 0.0,
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
                start_frame: rng.range(0.0, 4.0),
                frame_rate: 0.0,
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
/// Contact flash → dirt ejecta → tall dirt plume → low dust ring → lingering cloud on the shared
/// ring, plus one flat ground scar in its own ring. Physical scale (module doc), not screen-relative.
#[allow(clippy::too_many_arguments)]
fn spawn_big_splash(
    position: Vec3,
    normal: Vec3,
    to_camera: Vec3,
    assets: &ImpactAssets,
    materials: &mut Assets<VfxBillboardMaterial>,
    ring: &mut BillboardRing,
    ground_ring: &mut GroundMarkRing,
    rng: &mut ViewRng,
    commands: &mut Commands,
) {
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
            start_frame: 0.0,
            frame_rate: 0.0,
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
    // by EJECTA_UP_BIAS, then spread), each a dirt-recolored stretched streak flying at launch speed.
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
        spawn_billboard(
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
                start_frame: rng.range(0.0, 4.0),
                frame_rate: 0.0,
                start_size: length,
                end_size: length * 0.7,
                aspect: Vec3::new(EJECTA_WIDTH_RATIO, 1.0, 1.0),
                roll: 0.0,
                spin: 0.0,
                erosion_end: 1.0,
                rotation: Some(spark_orientation(dir, to_camera)),
            },
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
                start_frame: rng.range(0.0, 4.0),
                frame_rate: 0.0,
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

    // --- Low dust ring: a wide, ground-oriented skirt blooming outward flat off the strike.
    spawn_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.dirt_material(SPLASH_RING_ALPHA),
            lifetime: SPLASH_RING_LIFETIME,
            origin: position + normal * 0.1,
            drift: Vec3::ZERO,
            frames: 4,
            start_frame: rng.range(0.0, 4.0),
            frame_rate: 0.0,
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

    // --- Lingering cloud: the large, low-alpha brown haze that hangs after the fountain falls.
    spawn_billboard(
        commands,
        materials,
        ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.dirt_material(SPLASH_CLOUD_ALPHA),
            lifetime: SPLASH_CLOUD_LIFETIME,
            origin: position + normal * (SPLASH_CLOUD_SIZE.0 * 0.5) + Vec3::Y * 0.5,
            drift: Vec3::Y * (SPLASH_PLUME_RISE * 0.15),
            frames: 4,
            start_frame: rng.range(0.0, 4.0),
            frame_rate: 0.0,
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
            start_frame: 3.0,
            frame_rate: 0.0,
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

    use crate::vfx::billboard::{BILLBOARD_CAP, Billboard, FaceCamera};

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
        app.insert_resource(ImpactAssets {
            quad: Handle::default(),
            dust_atlas: Handle::default(),
            dust_lut: Handle::default(),
            ping_atlas: Handle::default(),
            spark_atlas: Handle::default(),
            spark_lut: Handle::default(),
            dirt_lut: Handle::default(),
            scar_lut: Handle::default(),
        });
        app
    }

    fn trigger_impact(app: &mut App, normal: Vec3, caliber: f32) {
        app.world_mut().trigger(Impact {
            position: Vec3::ZERO,
            normal,
            caliber,
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

    /// One MG-caliber impact spawns the full small read: a dust billow + a ping + 2–4 sparks, all on
    /// the shared billboard ring, and NO ground scar (scars are 88-only). The dust and ping are the
    /// aspect-1 camera facers; the sparks are the needle streaks.
    #[test]
    fn every_impact_spawns_the_layered_read() {
        let mut app = harness();
        trigger_impact(&mut app, Vec3::Y, MG_CALIBER);
        let total = billboards(&mut app);
        let spark_n = sparks(&mut app).len();
        assert!(
            (SPARK_COUNT.0 as usize..=SPARK_COUNT.1 as usize).contains(&spark_n),
            "spark count {spark_n} outside {SPARK_COUNT:?}"
        );
        assert_eq!(
            total,
            spark_n + 2,
            "layered read = dust + ping + {spark_n} sparks"
        );
        assert_eq!(
            app.world().resource::<GroundMarkRing>().0.len(),
            0,
            "the MG read leaves no ground scar"
        );
        // Exactly two camera-facing aspect-1 elements: the dust billow and the ping.
        let world = app.world_mut();
        let mut q = world.query_filtered::<&Billboard, With<FaceCamera>>();
        let facers = q.iter(world).filter(|b| b.aspect.x > 0.5).count();
        assert_eq!(facers, 2, "dust billow + ping face the camera");
    }

    /// The 88 (big caliber) lands the full terrain splash instead: contact flash, 8–12 dirt ejecta,
    /// 1–2 plumes, a dust ring, and a lingering cloud on the shared ring, PLUS exactly one flat
    /// ground scar in its OWN ring, lying on the surface (its +Z turned onto the normal).
    #[test]
    fn big_caliber_spawns_the_splash_stack() {
        let mut app = harness();
        trigger_impact(&mut app, Vec3::Y, BIG_CALIBER);

        // Exactly one ground scar, in its independent ring, flat on the terrain (not camera-facing).
        let ground = app.world().resource::<GroundMarkRing>().0.clone();
        assert_eq!(ground.len(), 1, "one 88 ground scar in its own ring");
        let scar = ground[0];
        assert!(
            app.world().get::<FaceCamera>(scar).is_none(),
            "the ground scar lies flat, never camera-facing"
        );
        let rot = app
            .world()
            .get::<Transform>(scar)
            .expect("scar transform")
            .rotation;
        assert!(
            rot.mul_vec3(Vec3::Z).dot(Vec3::Y) > 0.99,
            "the scar's face lies on the terrain normal"
        );

        // The shared-ring stack: flash(1) + ejecta(8–12) + plume(1–2) + ring(1) + cloud(1).
        let lo = 1 + EJECTA_COUNT.0 as usize + SPLASH_PLUME_COUNT.0 as usize + 2;
        let hi = 1 + EJECTA_COUNT.1 as usize + SPLASH_PLUME_COUNT.1 as usize + 2;
        let shared = app.world().resource::<BillboardRing>().0.len();
        assert!(
            (lo..=hi).contains(&shared),
            "big splash stack size {shared} outside {lo}..={hi}"
        );
        // Total billboards = the shared stack plus the one scar (its own ring).
        assert_eq!(billboards(&mut app), shared + 1, "stack + the ground scar");
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

    /// The sparks all kick AWAY from the surface (positive component along the hit normal), each
    /// streak elongated along its own flight direction. This is the `Impact.normal` consumption
    /// contract.
    #[test]
    fn impacts_spark_along_the_normal() {
        let mut app = harness();
        let normal = Vec3::new(0.3, 0.9, -0.1).normalize();
        trigger_impact(&mut app, normal, MG_CALIBER);
        let sparks = sparks(&mut app);
        assert!(!sparks.is_empty(), "an impact must throw sparks");
        for (drift, axis_alignment) in sparks {
            assert!(
                drift.normalize().dot(normal) > 0.0,
                "a spark must kick off the surface"
            );
            // The quad's +Y (its long axis) rides the flight direction — velocity elongation.
            assert!(
                axis_alignment > 0.99,
                "streak axis must ride the flight direction (dot {axis_alignment})"
            );
        }
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
}
