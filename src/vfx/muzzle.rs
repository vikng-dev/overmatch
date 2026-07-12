//! The 88's firing signature (survey tricks 1/2/3/5): a 1–2-FRAME billboard flash cluster (one
//! camera-facing core + two bore-aligned flame planes), a transient shadowless muzzle light
//! (first frame hottest, ~100 ms decay), and one lingering eroded smoke puff (~1 s). Strict
//! lifetime discipline on the flash — the craft canon is emphatic that a flash alive past ~2
//! frames reads slow and weak; the smoke and the light are what linger.
//!
//! Hook: an observer on the sim's [`FireShell`] event — the SAME seam the shell scene and the
//! tracer child hang off (`ballistics::on_fire_shell`), gated to the 88 by the SAME caliber
//! boundary (`ballistics::TRACER_MAX_CALIBER`). Both local fire (`shooting::fire`, FixedUpdate)
//! and remote fire (`net::client::receive_fire_events` re-raising `FireShell` in Update) arrive
//! here; `FireShell::origin`/`direction` are the muzzle pose at the fire tick on every path.
//!
//! Rollback honesty (the tracer-child precedent): a rollback replay that re-crosses the fire tick
//! re-runs `shooting::fire` and can re-trigger `FireShell`, duplicating the dressing exactly as it
//! duplicates the cosmetic shell + tracer child today. The dressing is idempotent in EFFECT —
//! sub-second lifetimes and the shared billboard/light rings bound any pile-up — so it follows the
//! precedent rather than inventing a dedup key the shell itself doesn't have.
//!
//! Staleness (survey trick 13): a remote shot arrives with `catch_up_ticks` of skipped flight; past
//! [`STALE_FIRE_TICKS`] (~250 ms) the flash moment is long over on the shooter's screen, so the
//! whole dressing is skipped rather than played late. Distance LOD: beyond [`FAR_FULL_DRESSING`]
//! only the core + light spawn (they carry the read at range; the planes and smoke are sub-pixel
//! overdraw there).

use bevy::prelude::*;

use crate::ballistics::{FireShell, TRACER_MAX_CALIBER};

use super::ViewRng;
use super::billboard::{
    BillboardRing, BillboardSpec, VfxBillboardMaterial, VfxParams, gradient_lut, spawn_billboard,
    unit_quad,
};

/// Flash cluster lifetime (s): ~2 frames at 60 Hz. THE knob the survey warns about — push it past
/// ~0.05 and the gun starts reading slow.
const FLASH_LIFETIME: f32 = 0.035;
/// Core flash size range (m, diameter at the muzzle; the 88's fireball is car-sized for a frame).
const FLASH_CORE_SIZE: (f32, f32) = (2.4, 3.2);
/// Directional flame-plane length range (m) and their width as a fraction of length.
const FLASH_PLANE_LENGTH: (f32, f32) = (3.0, 4.4);
const FLASH_PLANE_WIDTH_RATIO: f32 = 0.55;
/// Emissive boost on the flash LUT's heat lane — well above 1.0 so bloom catches the whole flash.
const FLASH_GLOW: f32 = 14.0;

/// Lingering muzzle smoke: lifetime (s), size ease (m), and its drift (up + a muzzle-gas push).
const SMOKE_LIFETIME: f32 = 1.2;
const SMOKE_SIZE: (f32, f32) = (1.6, 4.2);
const SMOKE_RISE: f32 = 0.55;
const SMOKE_PUSH: f32 = 1.3;
/// Slow flipbook playback for the smoke (frames/s over the 4-frame atlas) and its roll rate (rad/s).
const SMOKE_FRAME_RATE: f32 = 5.0;
const SMOKE_SPIN_MAX: f32 = 0.6;
/// Faint heat on young smoke (it is lit by the flash for the first instants).
const SMOKE_GLOW: f32 = 5.0;

/// Muzzle light: peak luminous power (lm), falloff range (m), decay time (s). First frame hottest,
/// gone in ~100 ms; never a shadow caster (the expensive half of a light).
const LIGHT_PEAK_LUMENS: f32 = 8.0e6;
const LIGHT_RANGE: f32 = 35.0;
const LIGHT_LIFETIME: f32 = 0.1;
/// Live muzzle-light ring cap — pathological-refire bound, same shape as the billboard ring.
const LIGHT_CAP: usize = 6;

/// A remote shot older than this many fixed ticks (~250 ms at 64 Hz) skips the dressing entirely
/// (survey: stale cosmetic events skip rather than play late).
const STALE_FIRE_TICKS: u32 = 16;
/// Beyond this camera distance (m) only the core + light spawn (LOD by distance — the cheap half
/// of the overdraw discipline).
const FAR_FULL_DRESSING: f32 = 400.0;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<MuzzleLightRing>()
        .add_systems(Startup, setup_muzzle_assets)
        .add_observer(on_main_gun_fire)
        .add_systems(Update, decay_muzzle_lights);
}

/// Preloaded muzzle-dressing assets: the shared quad, the two sprite atlases, and the per-effect
/// gradient LUTs (one grayscale sprite set, recolored per effect — the LUT trick).
#[derive(Resource)]
pub(super) struct MuzzleVfxAssets {
    pub(super) quad: Handle<Mesh>,
    pub(super) core_atlas: Handle<Image>,
    flame_atlas: Handle<Image>,
    smoke_atlas: Handle<Image>,
    flash_lut: Handle<Image>,
    smoke_lut: Handle<Image>,
}

impl MuzzleVfxAssets {
    /// The flash material template (additive — hot cores never darken; survey trick 9).
    pub(super) fn flash_material(
        &self,
        atlas: Handle<Image>,
        sharpness: f32,
    ) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, sharpness, 0.0, 1.0),
                glow: Vec4::new(FLASH_GLOW, 0.0, 0.0, 0.0),
            },
            atlas,
            lut: self.flash_lut.clone(),
            alpha_mode: AlphaMode::Add,
        }
    }

    /// The smoke material template (alpha-blend — smoke is mass, it darkens and occludes).
    pub(super) fn smoke_material(&self) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                // Moderate sharpness: soft dissolve edges on the puff.
                fade: Vec4::new(0.0, 2.6, 0.0, 0.85),
                glow: Vec4::new(SMOKE_GLOW, 0.0, 0.0, 0.0),
            },
            atlas: self.smoke_atlas.clone(),
            lut: self.smoke_lut.clone(),
            alpha_mode: AlphaMode::Blend,
        }
    }
}

pub(super) fn setup_muzzle_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Flash LUT: signal-hot core → orange edges, uniformly heat-loaded (the flash lives 2 frames —
    // the life axis barely matters). Rgb chosen so the ADDITIVE blend sums toward white-hot.
    let flash_lut = gradient_lut(&mut images, |x, _y| {
        let color = LinearRgba::rgb(0.9 + 0.1 * x, 0.35 + 0.6 * x * x, 0.08 + 0.5 * x * x * x);
        (color, 0.3 + 0.7 * x)
    });
    // Smoke LUT: warm powder-gray at birth cooling to a pale neutral, luminance riding the sprite
    // signal; heat only in the young, bright texels (flash-lit smoke blooms for the first instants,
    // then it is inert mass).
    let smoke_lut = gradient_lut(&mut images, |x, y| {
        let lum = 0.16 + 0.55 * x;
        let warm = (1.0 - y) * 0.25;
        let color = LinearRgba::rgb(lum * (0.9 + warm), lum * (0.86 + warm * 0.55), lum * 0.82);
        let heat = x * (-y * 9.0).exp();
        (color, heat)
    });
    commands.insert_resource(MuzzleVfxAssets {
        quad: unit_quad(&mut meshes),
        core_atlas: asset_server.load("vfx/flash_core.png"),
        flame_atlas: asset_server.load("vfx/flash_flames_atlas.png"),
        smoke_atlas: asset_server.load("vfx/smoke_atlas.png"),
        flash_lut,
        smoke_lut,
    });
}

/// A live muzzle light's age; [`decay_muzzle_lights`] drives the intensity fall and the despawn.
#[derive(Component)]
struct MuzzleLight {
    age: f32,
}

/// Live muzzle lights, oldest first — the refire leak bound (see [`LIGHT_CAP`]).
#[derive(Resource, Default)]
struct MuzzleLightRing(std::collections::VecDeque<Entity>);

/// Dress a main-gun shot: flash cluster + muzzle light + lingering smoke, all view entities hung
/// off the `FireShell` geometry (origin + bore direction). MG-calibre rounds pass through untouched
/// — their dressing is slice B, on this same machinery.
fn on_main_gun_fire(
    fire: On<FireShell>,
    assets: Res<MuzzleVfxAssets>,
    mut materials: ResMut<Assets<VfxBillboardMaterial>>,
    mut ring: ResMut<BillboardRing>,
    mut light_ring: ResMut<MuzzleLightRing>,
    mut rng: ResMut<ViewRng>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut commands: Commands,
) {
    // The same boundary as the shell-scene branch in `ballistics::on_fire_shell`: this dressing is
    // the main gun's.
    if fire.caliber < TRACER_MAX_CALIBER {
        return;
    }
    // Stale remote shot: the flash moment is long past — skip, don't play late.
    if fire.catch_up_ticks > STALE_FIRE_TICKS {
        return;
    }
    let origin = fire.origin;
    let dir = Vec3::from(fire.direction);
    // Distance LOD: with no camera (headless harness) treat as near.
    let near = camera
        .single()
        .map(|cam| {
            cam.translation().distance_squared(origin) < FAR_FULL_DRESSING * FAR_FULL_DRESSING
        })
        .unwrap_or(true);

    // --- Flash core: one camera-facing additive billboard, 1–2 frames. Random roll + size so no
    // two shots match; single-frame sprite, so no flipbook here.
    let core_size = rng.range(FLASH_CORE_SIZE.0, FLASH_CORE_SIZE.1);
    spawn_billboard(
        &mut commands,
        &mut materials,
        &mut ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.flash_material(assets.core_atlas.clone(), 2.0),
            lifetime: FLASH_LIFETIME,
            origin: origin + dir * 1.0,
            drift: Vec3::ZERO,
            frames: 1,
            start_frame: 0.0,
            frame_rate: 0.0,
            start_size: core_size,
            end_size: core_size * 1.25,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: 0.0,
            erosion_end: 0.0,
            rotation: None,
        },
    );

    // --- Directional flame planes: two bore-aligned quads, ~90° apart around the bore, each on a
    // random frame of the 4-flame atlas (the random-start-frame anti-repetition trick — per SHOT
    // here, since nothing lives long enough to animate).
    if near {
        let base_roll = rng.range(0.0, std::f32::consts::TAU);
        let plane_frame = rng.range(0.0, 4.0).floor();
        for i in 0..2 {
            let length = rng.range(FLASH_PLANE_LENGTH.0, FLASH_PLANE_LENGTH.1);
            // Sprite +Y (flame up) onto the bore, then rolled around the bore so the two planes
            // cross; the quad's center sits ~45% down the flame so the base hugs the muzzle.
            let rotation =
                Quat::from_axis_angle(dir, base_roll + i as f32 * std::f32::consts::FRAC_PI_2)
                    * Quat::from_rotation_arc(Vec3::Y, dir);
            spawn_billboard(
                &mut commands,
                &mut materials,
                &mut ring,
                assets.quad.clone(),
                BillboardSpec {
                    material: assets.flash_material(assets.flame_atlas.clone(), 2.0),
                    lifetime: FLASH_LIFETIME,
                    origin: origin + dir * (length * 0.45),
                    drift: Vec3::ZERO,
                    frames: 4,
                    start_frame: (plane_frame + i as f32) % 4.0,
                    frame_rate: 0.0,
                    start_size: length,
                    end_size: length * 1.15,
                    aspect: Vec3::new(FLASH_PLANE_WIDTH_RATIO, 1.0, 1.0),
                    roll: 0.0,
                    spin: 0.0,
                    erosion_end: 0.0,
                    rotation: Some(rotation),
                },
            );
        }
    }

    // --- Lingering smoke puff: one alpha-blended eroding billboard, random start frame/roll/spin,
    // rising and pushed along the bore by the muzzle gas.
    if near {
        spawn_billboard(
            &mut commands,
            &mut materials,
            &mut ring,
            assets.quad.clone(),
            BillboardSpec {
                material: assets.smoke_material(),
                lifetime: SMOKE_LIFETIME,
                origin: origin + dir * 1.6,
                drift: Vec3::Y * SMOKE_RISE + dir * SMOKE_PUSH,
                frames: 4,
                start_frame: rng.range(0.0, 4.0),
                frame_rate: SMOKE_FRAME_RATE,
                start_size: SMOKE_SIZE.0,
                end_size: SMOKE_SIZE.1,
                aspect: Vec3::ONE,
                roll: rng.range(0.0, std::f32::consts::TAU),
                spin: rng.range(-SMOKE_SPIN_MAX, SMOKE_SPIN_MAX),
                erosion_end: 1.0,
                rotation: None,
            },
        );
    }

    // --- Muzzle light: transient, shadowless, first frame hottest (Vlambeer: the environment
    // lighting up IS a large share of the perceived power).
    let light = commands
        .spawn((
            MuzzleLight { age: 0.0 },
            PointLight {
                color: Color::srgb(1.0, 0.72, 0.42),
                intensity: LIGHT_PEAK_LUMENS,
                range: LIGHT_RANGE,
                radius: 0.4,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_translation(origin + dir * 1.2),
        ))
        .id();
    light_ring.0.push_back(light);
    while light_ring.0.len() > LIGHT_CAP {
        if let Some(old) = light_ring.0.pop_front() {
            commands.entity(old).try_despawn();
        }
    }
}

/// Decay each muzzle light hard (cubic — most of the drop in the first frames) and despawn at
/// [`LIGHT_LIFETIME`].
fn decay_muzzle_lights(
    time: Res<Time>,
    mut lights: Query<(Entity, &mut MuzzleLight, &mut PointLight)>,
    mut commands: Commands,
) {
    for (entity, mut light, mut point) in &mut lights {
        light.age += time.delta_secs();
        let t = light.age / LIGHT_LIFETIME;
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let falloff = 1.0 - t;
        point.intensity = LIGHT_PEAK_LUMENS * falloff * falloff * falloff;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfx::billboard::Billboard;

    /// Minimal app carrying what the observer + agers read: bare asset stores, a fixed-seed view
    /// RNG, no camera (distance LOD treats that as near — full dressing).
    fn harness() -> App {
        let mut app = App::new();
        app.init_resource::<BillboardRing>()
            .init_resource::<MuzzleLightRing>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<VfxBillboardMaterial>>()
            .init_resource::<Time>()
            .insert_resource(ViewRng::seeded(42))
            .add_observer(on_main_gun_fire)
            .add_systems(Update, decay_muzzle_lights);
        app.insert_resource(MuzzleVfxAssets {
            quad: Handle::default(),
            core_atlas: Handle::default(),
            flame_atlas: Handle::default(),
            smoke_atlas: Handle::default(),
            flash_lut: Handle::default(),
            smoke_lut: Handle::default(),
        });
        app
    }

    fn fire(app: &mut App, caliber: f32, catch_up_ticks: u32) {
        app.world_mut().trigger(FireShell {
            origin: Vec3::new(1.0, 2.0, 3.0),
            direction: Dir3::X,
            speed: 773.0,
            caliber,
            mass: 10.2,
            shooter: None,
            tracer: true,
            catch_up_ticks,
        });
        app.world_mut().flush();
    }

    fn billboards(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<Billboard>>()
            .iter(app.world())
            .count()
    }

    fn lights(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<MuzzleLight>>()
            .iter(app.world())
            .count()
    }

    /// An 88 shot spawns the full dressing — core + 2 planes + smoke (4 billboards) and 1 light —
    /// and an MG-calibre round spawns NOTHING from this module (its dressing is slice B).
    #[test]
    fn main_gun_dresses_mg_does_not() {
        let mut app = harness();
        fire(&mut app, 0.088, 0);
        assert_eq!(billboards(&mut app), 4, "core + 2 planes + smoke");
        assert_eq!(lights(&mut app), 1);

        let mut mg = harness();
        fire(&mut mg, 0.0079, 0);
        assert_eq!(billboards(&mut mg), 0, "MG rounds get no 88 dressing");
        assert_eq!(lights(&mut mg), 0);
    }

    /// A stale remote shot (catch-up beyond ~250 ms) skips the dressing rather than playing late.
    #[test]
    fn stale_remote_fire_skips_dressing() {
        let mut app = harness();
        fire(&mut app, 0.088, STALE_FIRE_TICKS + 1);
        assert_eq!(billboards(&mut app), 0);
        assert_eq!(lights(&mut app), 0);
        // At or under the boundary the dressing still plays (~150 ms catch-up is the normal case).
        fire(&mut app, 0.088, STALE_FIRE_TICKS);
        assert_eq!(billboards(&mut app), 4);
    }

    /// The muzzle light decays monotonically from its first-frame peak and despawns at end of
    /// life — the "first frame hottest" contract.
    #[test]
    fn muzzle_light_decays_then_despawns() {
        let mut app = harness();
        fire(&mut app, 0.088, 0);
        let world = app.world_mut();
        let mut q = world.query::<(&MuzzleLight, &PointLight)>();
        let (_, point) = q.single(world).expect("one light");
        assert_eq!(point.intensity, LIGHT_PEAK_LUMENS, "born at peak");
        assert!(
            !point.shadow_maps_enabled,
            "muzzle light must never cast shadows"
        );

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(LIGHT_LIFETIME * 0.5));
        app.update();
        let world = app.world_mut();
        let mut q = world.query::<(&MuzzleLight, &PointLight)>();
        let (_, point) = q.single(world).expect("still alive mid-decay");
        assert!(
            point.intensity < LIGHT_PEAK_LUMENS * 0.5,
            "cubic decay front-loads the drop"
        );

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(LIGHT_LIFETIME));
        app.update();
        assert_eq!(lights(&mut app), 0, "expired light must despawn");
    }

    /// Refire storms are bounded: the light ring evicts oldest-first at its cap (the billboard
    /// ring's own cap is pinned in the billboard tests).
    #[test]
    fn light_ring_caps_refire() {
        let mut app = harness();
        for _ in 0..LIGHT_CAP + 5 {
            fire(&mut app, 0.088, 0);
        }
        assert_eq!(lights(&mut app), LIGHT_CAP);
    }
}
