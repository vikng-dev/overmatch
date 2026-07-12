//! View-layer impact VFX, layered per the survey's impact recipe (trick 7, scaled to what `Impact`
//! carries): a HYBRID read on every landed round —
//!   * a Kenney dust BILLOW (alpha-blend billboard, its own warm-gray→earth LUT) that expands and
//!     drifts along the surface normal — the mass of the read, replacing the old emissive sphere
//!     that read as a glowing circle rather than kicked dust;
//!   * a 1-frame additive PING (a small round glow) at the hit point — the instant of contact; and
//!   * a handful of stretched-billboard SPARKS (survey trick 8: velocity-elongated additive streaks,
//!     free motion blur) kicked in a cone around the hit's surface normal — the crisp hot garnish.
//! All three ride the shared billboard machinery ([`super::billboard`]) — one aging system, one
//! eviction ring — so an impact storm stays bounded with the muzzle dressing. The read exists so the
//! four non-tracer rounds of an MG belt cycle still register at the target: the rounds themselves
//! stay invisible in flight (only every fifth gets a streak — `ballistics::on_fire_shell`), but
//! every round that lands billows + pings + sparks, so a burst visibly walks across whatever it is
//! hitting instead of one lone tracer arriving out of nowhere.
//!
//! `Impact` deliberately stays lean (position + normal, no caliber), so the 88 and the MG share
//! one impact look; per-caliber/material differentiation (armor sparks vs dirt debris) is deferred
//! rather than growing the event.
//!
//! Strictly view-only (ADR-0014): subscribes to the sim's [`Impact`] event and spawns short-lived
//! render entities that no sim system ever reads — safe on a predicting net client (the replica
//! still flies cosmetic shells and sparks `Impact`s; damage authority is untouched). Mounted by both
//! windowed client compositions (SP `ClientPlugin` and `NetClientPlugin`); the headless server and
//! the scripted harness never mount it.

use bevy::prelude::*;

use crate::ballistics::Impact;

use super::ViewRng;
use super::billboard::{
    BillboardRing, BillboardSpec, VfxBillboardMaterial, VfxParams, gradient_lut, spawn_billboard,
    unit_quad,
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

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, setup_impact_assets)
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
    commands.insert_resource(ImpactAssets {
        quad: unit_quad(&mut meshes),
        dust_atlas: asset_server.load("vfx/impact_dust.png"),
        dust_lut,
        // A generic round glow (the same `light_01`-derived sprite the MG core uses; the asset
        // server dedupes the load and shares the GPU texture).
        ping_atlas: asset_server.load("vfx/mg_core.png"),
        spark_atlas: asset_server.load("vfx/spark_atlas.png"),
        spark_lut,
    });
}

/// Drop the layered read at each shell impact — every round, tracer or not: the impact is what makes
/// the four invisible non-tracer rounds of the belt cycle read at the target. Dust billow (mass) +
/// ping (contact flash) + a few sparks (crisp garnish), all on the shared billboard ring.
fn spawn_impact_read(
    impact: On<Impact>,
    assets: Res<ImpactAssets>,
    mut materials: ResMut<Assets<VfxBillboardMaterial>>,
    mut ring: ResMut<BillboardRing>,
    mut rng: ResMut<ViewRng>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut commands: Commands,
) {
    let normal = impact.normal.try_normalize().unwrap_or(Vec3::Y);

    // --- Dust billow: one alpha-blended eroding billboard, random frame/roll/spin, blooming and
    // drifting out along the surface normal (with a gentle rise).
    let dust_size = rng.range(DUST_SIZE.0, DUST_SIZE.1);
    spawn_billboard(
        &mut commands,
        &mut materials,
        &mut ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.dust_material(),
            lifetime: rng.range(DUST_LIFETIME.0, DUST_LIFETIME.1),
            origin: impact.position + normal * (DUST_SIZE.0 * 0.5),
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
        &mut commands,
        &mut materials,
        &mut ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.ping_material(),
            lifetime: PING_LIFETIME,
            origin: impact.position + normal * 0.05,
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
    let to_camera = camera
        .single()
        .map(|cam| cam.translation() - impact.position)
        .unwrap_or(Vec3::Z);
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
            &mut commands,
            &mut materials,
            &mut ring,
            assets.quad.clone(),
            BillboardSpec {
                material: assets.spark_material(),
                lifetime: rng.range(SPARK_LIFETIME.0, SPARK_LIFETIME.1),
                origin: impact.position + dir * (length * 0.5),
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

    /// Minimal app carrying what the impact observer reads. Real `Assets` stores (initialized bare,
    /// no asset plugins) so the per-billboard material clones run for real; fixed-seed view RNG; no
    /// camera (spark facing falls back).
    fn harness() -> App {
        let mut app = App::new();
        app.init_resource::<BillboardRing>()
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
        });
        app
    }

    fn trigger_impact(app: &mut App, normal: Vec3) {
        app.world_mut().trigger(Impact {
            position: Vec3::ZERO,
            normal,
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

    /// One impact spawns the full layered read: a dust billow + a ping + 2–4 sparks, all on the
    /// shared billboard ring. The dust and ping are the aspect-1 camera facers; the sparks are the
    /// needle streaks.
    #[test]
    fn every_impact_spawns_the_layered_read() {
        let mut app = harness();
        trigger_impact(&mut app, Vec3::Y);
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
        // Exactly two camera-facing aspect-1 elements: the dust billow and the ping.
        let world = app.world_mut();
        let mut q = world.query_filtered::<&Billboard, With<FaceCamera>>();
        let facers = q.iter(world).filter(|b| b.aspect.x > 0.5).count();
        assert_eq!(facers, 2, "dust billow + ping face the camera");
    }

    /// The sparks all kick AWAY from the surface (positive component along the hit normal), each
    /// streak elongated along its own flight direction. This is the `Impact.normal` consumption
    /// contract.
    #[test]
    fn impacts_spark_along_the_normal() {
        let mut app = harness();
        let normal = Vec3::new(0.3, 0.9, -0.1).normalize();
        trigger_impact(&mut app, normal);
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
        trigger_impact(&mut app, Vec3::ZERO);
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
            trigger_impact(&mut app, Vec3::Y);
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
