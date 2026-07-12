//! View-layer impact VFX, layered per the survey's impact recipe (trick 7, scaled to what `Impact`
//! carries): (a) the small emissive dust puff at EVERY shell impact — the ship-facing sibling of
//! the dev-only `debug::spawn_impact_marker` (same `Impact` subscription, same preloaded-assets +
//! ring-buffer-eviction shape, but always on and animated instead of gizmo-gated and static) — and
//! (b) a handful of stretched-billboard SPARKS (survey trick 8: velocity-elongated additive
//! streaks, free motion blur) kicked in a cone around the hit's surface normal, the crisp half of
//! the read. The puff exists so the four non-tracer rounds of an MG belt cycle still READ at the
//! target: the rounds themselves stay invisible in flight (only every fifth gets a streak —
//! `ballistics::on_fire_shell`), but every round that lands puffs + sparks, so a burst visibly
//! walks across whatever it is hitting instead of one lone tracer arriving out of nowhere.
//!
//! `Impact` deliberately stays lean (position + normal, no caliber), so the 88 and the MG share
//! one impact look; per-caliber differentiation is deferred rather than growing the event.
//!
//! Strictly view-only (ADR-0014): subscribes to the sim's [`Impact`] event and spawns short-lived
//! render entities that no sim system ever reads — safe on a predicting net client (the replica
//! still flies cosmetic shells and sparks `Impact`s; damage authority is untouched). Mounted by both
//! windowed client compositions (SP `ClientPlugin` and `NetClientPlugin`); the headless server and
//! the scripted harness never mount it.

use std::collections::VecDeque;

use bevy::color::Alpha;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;

use crate::ballistics::Impact;

use super::ViewRng;
use super::billboard::{
    BillboardRing, BillboardSpec, VfxBillboardMaterial, VfxParams, gradient_lut, spawn_billboard,
    unit_quad,
};

/// Puff lifetime (s): the whole expand-and-fade. Short — a spark of dust kicked off the surface,
/// not a smoke column; at the MG's cyclic rate anything longer starts stacking into fog.
const PUFF_LIFETIME: f32 = 0.35;
/// Sphere mesh radius (m) at scale 1 — the puff's spawn size.
const PUFF_RADIUS: f32 = 0.12;
/// Uniform scale the puff expands to by the end of its life (spawn size × this).
const PUFF_END_SCALE: f32 = 3.0;
/// Live-puff ring cap. Both MGs cycling (~25 impacts/s) at `PUFF_LIFETIME` keep ~9 alive, so the
/// cap is headroom for bursts/spall pileups, not steady state — the eviction is a leak bound, the
/// same job `debug::IMPACT_MARKER_CAP` does for the debug markers.
const PUFF_CAP: usize = 64;
/// The puff's emissive at birth (linear, above 1.0 so bloom catches it) — warm dust/spark, kept
/// well below the tracer streak's `LinearRgba::rgb(30, 12, 3)` so impacts read as secondary flashes,
/// not competing tracers.
const PUFF_EMISSIVE: LinearRgba = LinearRgba {
    red: 8.0,
    green: 5.0,
    blue: 3.0,
    alpha: 1.0,
};

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
    app.init_resource::<PuffRing>()
        .add_systems(Startup, setup_puff_assets)
        // The ship impact puff: a view-side subscriber to `ballistics`' sim `Impact` event
        // (ADR-0014) — the same seam the debug marker and the sandbox subscribe to.
        .add_observer(spawn_impact_puff)
        .add_systems(Update, age_impact_puffs);
}

/// Preloaded puff view assets: the shared mesh handle plus the birth-state MATERIAL VALUE (not a
/// handle — each puff clones it into its own asset so the per-frame fade can mutate one puff
/// without fading every other live puff in lockstep). `pub(super)` so the prewarm rig can warm this
/// exact mesh + material combination's pipeline at startup.
#[derive(Resource)]
pub(super) struct PuffAssets {
    pub(super) mesh: Handle<Mesh>,
    pub(super) material: StandardMaterial,
}

/// Live puffs in spawn order, oldest at the front — evicted past [`PUFF_CAP`], exactly the
/// `debug::ImpactMarkerRing` shape. Naturally-expired puffs leave stale entries behind; eviction
/// uses `try_despawn`, so a stale entry is a silent no-op.
#[derive(Resource, Default)]
struct PuffRing(VecDeque<Entity>);

/// A live impact puff's age (s); [`age_impact_puffs`] drives the expand/fade from it and despawns
/// the puff at [`PUFF_LIFETIME`].
#[derive(Component)]
struct ImpactPuff {
    age: f32,
}

/// Preloaded spark view assets: the shared quad plus the atlas/LUT the spark material template is
/// built from. The atlas is the muzzle flash's core flare loaded by path (the asset server dedupes
/// the load); the LUT is the sparks' own palette. `pub(super)` so the prewarm rig can warm the
/// spark billboard's texture bindings at startup.
#[derive(Resource)]
pub(super) struct SparkAssets {
    pub(super) quad: Handle<Mesh>,
    atlas: Handle<Image>,
    lut: Handle<Image>,
}

impl SparkAssets {
    /// The spark material template: additive (hot metal never darkens), single-frame full-image
    /// lanes (the flare sprite is the whole texture), hard erosion edge so the streak dissolves
    /// crisply instead of ghosting.
    pub(super) fn spark_material(&self) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 1.0, 1.0, 0.0),
                fade: Vec4::new(0.0, 3.0, 0.0, 1.0),
                glow: Vec4::new(SPARK_GLOW, 0.0, 0.0, 0.0),
            },
            atlas: self.atlas.clone(),
            lut: self.lut.clone(),
            alpha_mode: AlphaMode::Add,
        }
    }
}

pub(super) fn setup_puff_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
) {
    commands.insert_resource(PuffAssets {
        mesh: meshes.add(Sphere::new(PUFF_RADIUS)),
        // Same no-lit-contribution recipe as the tracer streak (black base, zero reflectance): the
        // emissive is the whole visual. `Blend` so the fade can sink it into the background —
        // Bevy scales the emissive contribution by the diffuse alpha under Blend, so the alpha fade
        // below fades the glow too, not just a black shell.
        material: StandardMaterial {
            base_color: Color::BLACK,
            reflectance: 0.0,
            emissive: PUFF_EMISSIVE,
            alpha_mode: AlphaMode::Blend,
            ..default()
        },
    });
    // Spark LUT: white-yellow hot at birth cooling toward ember orange, heat front-loaded so young
    // sparks bloom and old ones die as dim embers.
    let spark_lut = gradient_lut(&mut images, |x, y| {
        let cool = 1.0 - y;
        let color = LinearRgba::rgb(x, x * (0.55 + 0.35 * cool), x * x * (0.12 + 0.5 * cool));
        (color, x * (0.35 + 0.65 * cool))
    });
    commands.insert_resource(SparkAssets {
        quad: unit_quad(&mut meshes),
        atlas: asset_server.load("vfx/flash_core.png"),
        lut: spark_lut,
    });
}

/// Drop a puff + a few sparks at each shell impact — every round, tracer or not: the impact is
/// what makes the four invisible non-tracer rounds of the belt cycle read at the target. The puff
/// is the mass (alpha-blend, soft); the sparks are the crisp hot garnish (additive streaks off the
/// surface normal).
fn spawn_impact_puff(
    impact: On<Impact>,
    assets: Res<PuffAssets>,
    sparks: Res<SparkAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut billboard_materials: ResMut<Assets<VfxBillboardMaterial>>,
    mut ring: ResMut<PuffRing>,
    mut billboard_ring: ResMut<BillboardRing>,
    mut rng: ResMut<ViewRng>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut commands: Commands,
) {
    // Per-puff material asset (see `PuffAssets::material`): the fade mutates it every frame. The
    // strong handle lives only on the puff entity, so despawning the puff frees the asset.
    let material = materials.add(assets.material.clone());
    let puff = commands
        .spawn((
            ImpactPuff { age: 0.0 },
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(material),
            Transform::from_translation(impact.position),
            // A glow puff neither casts nor receives shadow (same rule as the tracer streak).
            NotShadowCaster,
            NotShadowReceiver,
        ))
        .id();
    ring.0.push_back(puff);
    // Evict from the front until back under the cap (`try_despawn`: stale/already-expired entries
    // are silent no-ops).
    while ring.0.len() > PUFF_CAP {
        if let Some(old) = ring.0.pop_front() {
            commands.entity(old).try_despawn();
        }
    }

    // --- Sparks: 2–4 stretched additive streaks in a cone around the surface normal (a degenerate
    // normal falls back to straight up — sparks off the ground still read). Each is a fixed-
    // orientation billboard elongated along its own flight direction, drifting at launch speed;
    // erosion + the LUT's cooling row kill it as a dim ember. They ride the shared billboard ring,
    // so refire storms stay bounded with the muzzle dressing.
    let normal = impact.normal.try_normalize().unwrap_or(Vec3::Y);
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
            &mut billboard_materials,
            &mut billboard_ring,
            sparks.quad.clone(),
            BillboardSpec {
                material: sparks.spark_material(),
                lifetime: rng.range(SPARK_LIFETIME.0, SPARK_LIFETIME.1),
                origin: impact.position + dir * (length * 0.5),
                drift: dir * speed,
                frames: 1,
                start_frame: 0.0,
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

/// Expand and fade each live puff over its lifetime, despawning it at the end: scale runs
/// 1 → [`PUFF_END_SCALE`] while alpha and emissive run down to zero (the alpha also scales the
/// emissive under `Blend`, so the glow eases out rather than cutting).
fn age_impact_puffs(
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut puffs: Query<(
        Entity,
        &mut ImpactPuff,
        &mut Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    mut commands: Commands,
) {
    for (entity, mut puff, mut transform, material) in &mut puffs {
        puff.age += time.delta_secs();
        let t = puff.age / PUFF_LIFETIME;
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        transform.scale = Vec3::splat(1.0 + (PUFF_END_SCALE - 1.0) * t);
        if let Some(mut mat) = materials.get_mut(&material.0) {
            let fade = 1.0 - t;
            mat.base_color = mat.base_color.with_alpha(fade);
            mat.emissive = PUFF_EMISSIVE * fade;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::vfx::billboard::Billboard;

    /// Minimal app carrying what the puff + spark observer and the ager read. Real `Assets` stores
    /// (initialized bare, no asset plugins) so the per-puff material clone and the fade mutation
    /// run for real; fixed-seed view RNG; no camera (spark facing falls back).
    fn harness() -> App {
        let mut app = App::new();
        app.init_resource::<PuffRing>()
            .init_resource::<BillboardRing>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<VfxBillboardMaterial>>()
            .init_resource::<Time>()
            .insert_resource(ViewRng::seeded(42))
            .add_observer(spawn_impact_puff)
            .add_systems(Update, age_impact_puffs);
        app.insert_resource(PuffAssets {
            mesh: Handle::default(),
            material: StandardMaterial {
                emissive: PUFF_EMISSIVE,
                ..default()
            },
        });
        app.insert_resource(SparkAssets {
            quad: Handle::default(),
            atlas: Handle::default(),
            lut: Handle::default(),
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

    fn puff_count(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<ImpactPuff>>()
            .iter(app.world())
            .count()
    }

    fn spark_count(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<Billboard>>()
            .iter(app.world())
            .count()
    }

    #[test]
    fn every_impact_spawns_a_puff_and_ring_caps() {
        let mut app = harness();
        // Fire past the cap; the ring must hold at exactly the cap, oldest evicted — the same
        // leak bound the debug marker ring pins.
        for _ in 0..PUFF_CAP + 7 {
            trigger_impact(&mut app, Vec3::Y);
        }
        assert_eq!(puff_count(&mut app), PUFF_CAP);
        assert_eq!(app.world().resource::<PuffRing>().0.len(), PUFF_CAP);
    }

    /// One impact adds 2–4 sparks; the sparks all kick AWAY from the surface (positive component
    /// along the hit normal), each streak elongated along its own flight direction. This is the
    /// `Impact.normal` consumption contract.
    #[test]
    fn impacts_spark_along_the_normal() {
        let mut app = harness();
        let normal = Vec3::new(0.3, 0.9, -0.1).normalize();
        trigger_impact(&mut app, normal);
        let n = spark_count(&mut app);
        assert!(
            (SPARK_COUNT.0 as usize..=SPARK_COUNT.1 as usize).contains(&n),
            "spark count {n} outside {SPARK_COUNT:?}"
        );
        let world = app.world_mut();
        let mut q = world.query::<(&Billboard, &Transform)>();
        for (spark, transform) in q.iter(world) {
            let along = spark.drift.normalize().dot(normal);
            assert!(
                along > 0.0,
                "a spark must kick off the surface, got {along}"
            );
            assert!(
                spark.aspect.x < 0.2,
                "sparks are needle streaks (width ratio {})",
                spark.aspect.x
            );
            // The quad's +Y (its long axis) is the flight direction — velocity elongation.
            let quad_y = transform.rotation * Vec3::Y;
            let aligned = quad_y.dot(spark.drift.normalize());
            assert!(
                aligned > 0.99,
                "streak axis must ride the flight direction (dot {aligned})"
            );
        }
    }

    /// A degenerate (zero) normal still sparks — straight up, the terrain fallback — so the effect
    /// can never panic or vanish on a weird raycast.
    #[test]
    fn degenerate_normal_falls_back_up() {
        let mut app = harness();
        trigger_impact(&mut app, Vec3::ZERO);
        assert!(spark_count(&mut app) >= SPARK_COUNT.0 as usize);
        let world = app.world_mut();
        let mut q = world.query::<&Billboard>();
        for spark in q.iter(world) {
            assert!(spark.drift.y > 0.0, "fallback sparks kick upward");
        }
    }

    /// Sparks ride the shared billboard ring: an impact storm is bounded by its cap, not unbounded
    /// entity growth (the same leak bound every other vfx layer pins).
    #[test]
    fn spark_storm_is_ring_capped() {
        let mut app = harness();
        for _ in 0..200 {
            trigger_impact(&mut app, Vec3::Y);
        }
        let ring_len = app.world().resource::<BillboardRing>().0.len();
        assert_eq!(
            spark_count(&mut app),
            ring_len,
            "live sparks == ring entries"
        );
        assert!(
            ring_len <= crate::vfx::billboard::BILLBOARD_CAP,
            "spark storm must stay ring-capped (got {ring_len})"
        );
    }

    #[test]
    fn puffs_expire_after_lifetime() {
        let mut app = harness();
        trigger_impact(&mut app, Vec3::Y);
        assert_eq!(puff_count(&mut app), 1);
        // Advance time past the lifetime; the ager must despawn the puff.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(PUFF_LIFETIME + 0.05));
        app.update();
        assert_eq!(puff_count(&mut app), 0, "an expired puff must despawn");
    }

    /// Mid-life, the puff must have EXPANDED (scale > 1) and FADED (alpha + emissive below birth) —
    /// the two halves of the read.
    #[test]
    fn puffs_expand_and_fade_over_life() {
        let mut app = harness();
        trigger_impact(&mut app, Vec3::Y);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(PUFF_LIFETIME * 0.5));
        app.update();
        let world = app.world_mut();
        let mut q = world.query::<(&Transform, &MeshMaterial3d<StandardMaterial>, &ImpactPuff)>();
        let (transform, material, _) = q.single(world).expect("one live puff");
        assert!(
            transform.scale.x > 1.0,
            "mid-life puff must have expanded (scale {})",
            transform.scale.x
        );
        let handle = material.0.clone();
        let mat = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .expect("puff material asset");
        assert!(
            mat.base_color.alpha() < 1.0,
            "mid-life puff must have faded (alpha {})",
            mat.base_color.alpha()
        );
        assert!(
            mat.emissive.red < PUFF_EMISSIVE.red,
            "mid-life puff emissive must have dimmed (red {})",
            mat.emissive.red
        );
    }
}
