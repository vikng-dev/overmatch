//! View-only muzzle dressing for [`FireShell`] events.
//!
//! Invariant (ADR-0014): spawned entities are render-only. Light and billboard rings bound replay
//! duplicates; stale remote shots do not replay an already-expired flash.

use bevy::prelude::*;

// `STALE_FIRE_TICKS` is shared with the sim-side catch-up impact gate (`ballistics::on_fire_shell`)
// so the muzzle flash and the impact phantom fall stale together — one constant, no drift.
use crate::ballistics::{FireShell, STALE_FIRE_TICKS, TRACER_MAX_CALIBER};

use super::ViewRng;
use super::billboard::{
    BillboardRing, BillboardSpec, VfxBillboardMaterial, VfxParams, gradient_lut, smoothstep,
    spawn_billboard, unit_quad,
};

/// Flash cluster lifetime in seconds.
const FLASH_LIFETIME: f32 = 0.035;
/// Core flash diameter range in metres.
const FLASH_CORE_SIZE: (f32, f32) = (3.5, 4.6);
/// Directional flame-plane length range in metres and width ratio.
const FLASH_PLANE_LENGTH: (f32, f32) = (4.3, 6.4);
const FLASH_PLANE_WIDTH_RATIO: f32 = 0.55;
/// Emissive boost on the flash LUT's heat lane — well above 1.0 so bloom catches the whole flash.
const FLASH_GLOW: f32 = 14.0;

/// The 88's fireball glow card: ONE soft additive billboard behind the starburst core — the classic
/// card that sells fireball VOLUME between the 2-frame flash and the lingering smoke. Camera-facing,
/// ~1.5× the core, LOW alpha, on the round-glow (`mg_core`) sprite, and it fades fast over its own
/// ~0.1 s. It is the ONLY thing allowed to linger past [`FLASH_LIFETIME`] — the 2-frame flash
/// discipline itself is untouched. Rides the same additive billboard pipeline as the flash (no new
/// permutation, so the prewarm rig already covers it).
const FLASH_GLOW_CARD_SCALE: f32 = 1.5;
const FLASH_GLOW_CARD_LIFETIME: f32 = 0.1;
/// LOW overall alpha (billboard `fade.w`) and a softened emissive boost (`glow.x`) — a fill glow, not
/// a second hot core.
const FLASH_GLOW_CARD_ALPHA: f32 = 0.35;
const FLASH_GLOW_CARD_GLOW: f32 = 4.0;

/// Lingering muzzle smoke: lifetime (s), size ease (m), and its drift (up + a muzzle-gas push).
/// Birth size nudged up (was 1.6) so the punched-up flash hands off to a smoke puff that is already
/// present, not a wisp.
const SMOKE_LIFETIME: f32 = 1.2;
const SMOKE_SIZE: (f32, f32) = (2.2, 4.2);
const SMOKE_RISE: f32 = 0.55;
const SMOKE_PUSH: f32 = 1.3;
/// Slow flipbook playback for the smoke (frames/s over the 4-frame atlas) and its roll rate (rad/s).
const SMOKE_FRAME_RATE: f32 = 5.0;
const SMOKE_SPIN_MAX: f32 = 0.6;
/// Faint heat on young smoke (it is lit by the flash for the first instants).
const SMOKE_GLOW: f32 = 5.0;

/// Main-gun light peak (lm), range (m), and lifetime (s).
const LIGHT_PEAK_LUMENS: f32 = 8.0e6;
const LIGHT_RANGE: f32 = 35.0;
const LIGHT_LIFETIME: f32 = 0.1;
/// Shared muzzle-light population cap; oldest lights are evicted first.
const LIGHT_CAP: usize = 12;
/// The MG tracer-round brightness spike: a tracer round's muzzle light is this much brighter than a
/// ball round's, so the flicker still reads harder exactly when a streak leaves the barrel.
const MG_TRACER_LIGHT_BOOST: f32 = 1.5;
/// [`MuzzleShadows::MgEveryNth`] fallback: only every this-many-th MG light casts a shadow (the 88
/// always does in that mode). The measurement fallback if sustained MG shadow cost spikes.
const MG_SHADOW_EVERY: u32 = 4;

// --- The MG's dressing knobs (slice B): the 88's machinery at rifle scale.

/// MG flash lifetime (s): ~1–2 frames — even tighter than the 88's (a small flash that lingers
/// reads as a sputtering candle, not gunfire).
const MG_FLASH_LIFETIME: f32 = 0.03;
/// MG core flash size range (m) — a rifle-calibre pop, an order of magnitude under the 88's
/// fireball. The RANGE is also the per-shot size jitter.
const MG_FLASH_CORE_SIZE: (f32, f32) = (0.3, 0.55);
/// The single near-only MG flame plane: length range (m); shares the 88's width ratio.
const MG_FLASH_PLANE_LENGTH: (f32, f32) = (0.45, 0.8);
/// MG muzzle light — now on EVERY round (a per-round light at 750 rpm reads as a continuous muzzle
/// glimmer; the tracer round spikes [`MG_TRACER_LIGHT_BOOST`]× brighter so the streak still pops).
/// Dimmer, shorter, tighter than the 88's.
const MG_LIGHT_PEAK_LUMENS: f32 = 1.2e6;
const MG_LIGHT_RANGE: f32 = 16.0;
const MG_LIGHT_LIFETIME: f32 = 0.05;
/// MG smoke ration: one faint puff every this many MG rounds (across both guns — it is cosmetic
/// cadence, not per-barrel state). Per-round puffs at the cyclic rate are the overdraw trap.
const MG_SMOKE_EVERY: u32 = 4;
/// The MG puff: shorter, smaller, fainter than the 88's (alpha multiplier well under the 88's
/// 0.85), with a gentler rise and muzzle-gas push.
const MG_SMOKE_LIFETIME: f32 = 0.7;
const MG_SMOKE_SIZE: (f32, f32) = (0.3, 1.0);
const MG_SMOKE_ALPHA: f32 = 0.45;
const MG_SMOKE_RISE: f32 = 0.4;
const MG_SMOKE_PUSH: f32 = 0.7;

/// Beyond this camera distance (m) only the core + light spawn (LOD by distance — the cheap half
/// of the overdraw discipline).
const FAR_FULL_DRESSING: f32 = 400.0;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<MuzzleLightRing>()
        .init_resource::<MgSmokeCadence>()
        .init_resource::<MgShadowCadence>()
        .insert_resource(MuzzleShadows::from_env())
        .add_systems(Startup, setup_muzzle_assets)
        .add_observer(on_main_gun_fire)
        .add_observer(on_mg_fire)
        .add_systems(Update, decay_muzzle_lights);
}

/// Muzzle-light shadow policy, read once at plugin setup.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum MuzzleShadows {
    /// Every muzzle light casts a shadow (the decision's default).
    #[default]
    On,
    /// The 88 casts; MG lights cast only every [`MG_SHADOW_EVERY`]-th round (the cost fallback).
    MgEveryNth,
    /// No muzzle light casts a shadow (the measurement baseline / hard fallback).
    Off,
}

impl MuzzleShadows {
    fn from_env() -> Self {
        match std::env::var("OVERMATCH_MUZZLE_SHADOWS").ok().as_deref() {
            Some("off") => Self::Off,
            Some("mg-nth") => Self::MgEveryNth,
            // Unset, "on", or anything else: the default decision.
            _ => Self::On,
        }
    }
}

/// Preloaded muzzle-dressing assets: the shared quad, the two sprite atlases, and the per-effect
/// gradient LUTs (one grayscale sprite set, recolored per effect — the LUT trick).
#[derive(Resource)]
pub(super) struct MuzzleVfxAssets {
    pub(super) quad: Handle<Mesh>,
    /// The 88's flash core: a 2×2 scorch-starburst atlas (a spiky radial star per shot).
    pub(super) core_atlas: Handle<Image>,
    /// The MG's flash core: a single small round glow (`light_01`) — a rifle-scale pop, not the 88's
    /// starburst.
    mg_core: Handle<Image>,
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
                // glow.y = 1.0: the additive blend contract (see `vfx_billboard.wgsl`) — premultiply
                // by coverage so transparent texels add nothing (kills the old orange square).
                glow: Vec4::new(FLASH_GLOW, 1.0, 0.0, 0.0),
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
                // Moderate sharpness: soft dissolve edges on the puff. w is the 88's overall alpha
                // (nudged up from 0.85 so early smoke has more presence for the flash to hand off to);
                // the MG puff overrides it down to `MG_SMOKE_ALPHA`.
                fade: Vec4::new(0.0, 2.6, 0.0, 0.92),
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
    // Belt-and-braces against the premultiply bug: floor the color to BLACK at signal 0 (the
    // `smoothstep`-shaped ramp over the first ~17% of signal) so even a partial-alpha edge texel
    // reading its LUT floor contributes nothing — the additive premultiply already masks fully
    // transparent texels, this catches the anti-aliased fringe.
    let flash_lut = gradient_lut(&mut images, |x, _y| {
        let floor = smoothstep(0.0, 0.17, x);
        let color =
            LinearRgba::rgb(0.9 + 0.1 * x, 0.35 + 0.6 * x * x, 0.08 + 0.5 * x * x * x) * floor;
        (color, (0.3 + 0.7 * x) * floor)
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
        core_atlas: asset_server.load("vfx/flash_core_atlas.png"),
        mg_core: asset_server.load("vfx/mg_core.png"),
        flame_atlas: asset_server.load("vfx/flash_flames_atlas.png"),
        smoke_atlas: asset_server.load("vfx/smoke_atlas.png"),
        flash_lut,
        smoke_lut,
    });
}

/// A live muzzle light's age plus the scale it was born with; [`decay_muzzle_lights`] drives the
/// intensity fall and the despawn from these. Per-light `peak`/`lifetime` is what lets the 88 and
/// the MGs share one decay system at different scales.
#[derive(Component)]
struct MuzzleLight {
    age: f32,
    /// Peak intensity (lm) the cubic decay falls from.
    peak: f32,
    /// Seconds from peak to despawn.
    lifetime: f32,
}

/// Live muzzle lights, oldest first — the refire leak bound (see [`LIGHT_CAP`]).
#[derive(Resource, Default)]
struct MuzzleLightRing(std::collections::VecDeque<Entity>);

/// Belt-position counter for the MG smoke ration ([`MG_SMOKE_EVERY`]); ticks once per MG round.
#[derive(Resource, Default)]
struct MgSmokeCadence(u32);

/// Round counter for the [`MuzzleShadows::MgEveryNth`] fallback ([`MG_SHADOW_EVERY`]); ticks once
/// per MG round, deciding which rounds' lights cast a shadow in that mode.
#[derive(Resource, Default)]
struct MgShadowCadence(u32);

/// Spawn one transient muzzle light into the shared ring — the 88's and the MGs' common machinery;
/// peak/range/lifetime/radius are the caller's scale knobs, `shadows` the lever-resolved decision.
fn spawn_muzzle_light(
    commands: &mut Commands,
    ring: &mut MuzzleLightRing,
    position: Vec3,
    peak: f32,
    range: f32,
    lifetime: f32,
    radius: f32,
    shadows: bool,
) {
    let light = commands
        .spawn((
            MuzzleLight {
                age: 0.0,
                peak,
                lifetime,
            },
            PointLight {
                color: Color::srgb(1.0, 0.72, 0.42),
                intensity: peak,
                range,
                radius,
                // Direction-less point (the hull occludes it like any object); shadow casting is the
                // lever's call (see [`MuzzleShadows`]) — the expensive half of the light.
                shadow_maps_enabled: shadows,
                ..default()
            },
            Transform::from_translation(position),
        ))
        .id();
    ring.0.push_back(light);
    while ring.0.len() > LIGHT_CAP {
        if let Some(old) = ring.0.pop_front() {
            commands.entity(old).try_despawn();
        }
    }
}

/// Dress a main-gun shot: flash cluster + muzzle light + lingering smoke, all view entities hung
/// off the `FireShell` geometry (origin + bore direction). MG-calibre rounds pass through untouched
/// — their dressing is slice B, on this same machinery.
fn on_main_gun_fire(
    fire: On<FireShell>,
    assets: Res<MuzzleVfxAssets>,
    mut materials: ResMut<Assets<VfxBillboardMaterial>>,
    mut ring: ResMut<BillboardRing>,
    mut light_ring: ResMut<MuzzleLightRing>,
    shadows: Res<MuzzleShadows>,
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

    // --- Flash core: one camera-facing additive billboard, 1–2 frames. A random one of the four
    // scorch-starburst atlas frames (the anti-strobe frame pick — nothing lives long enough to
    // animate) plus random roll + size, so no two shots match.
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
            frames: 4,
            start_frame: rng.range(0.0, 4.0).floor(),
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

    // --- Fireball glow card: one soft additive round-glow billboard behind the core, LOW alpha and
    // a softened boost, ~1.5× the core. It is the one element allowed to outlive the 2-frame flash
    // (its own ~0.1 s, eroding out fast) — the volume that bridges the flash and the smoke. Near-only
    // dressing: beyond FAR_FULL_DRESSING only the core + light carry the read (see the module LOD
    // contract), so the glow card is gated with the planes and smoke below.
    if near {
        let mut glow = assets.flash_material(assets.mg_core.clone(), 1.5);
        glow.params.frame = Vec4::new(0.0, 1.0, 1.0, 0.0);
        glow.params.glow.x = FLASH_GLOW_CARD_GLOW;
        glow.params.fade.w = FLASH_GLOW_CARD_ALPHA;
        let glow_size = core_size * FLASH_GLOW_CARD_SCALE;
        spawn_billboard(
            &mut commands,
            &mut materials,
            &mut ring,
            assets.quad.clone(),
            BillboardSpec {
                material: glow,
                lifetime: FLASH_GLOW_CARD_LIFETIME,
                origin: origin + dir * 1.0,
                drift: Vec3::ZERO,
                frames: 1,
                start_frame: 0.0,
                frame_rate: 0.0,
                start_size: glow_size,
                end_size: glow_size * 1.3,
                aspect: Vec3::ONE,
                roll: rng.range(0.0, std::f32::consts::TAU),
                spin: 0.0,
                // Erode out fast over its short life — this is the "fade" that lets it linger without
                // the flash lingering.
                erosion_end: 1.0,
                rotation: None,
            },
        );
    }

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

    // --- Muzzle light: transient, first frame hottest (Vlambeer: the environment lighting up IS a
    // large share of the perceived power). The 88 casts a shadow unless the lever is fully Off.
    spawn_muzzle_light(
        &mut commands,
        &mut light_ring,
        origin + dir * 1.2,
        LIGHT_PEAK_LUMENS,
        LIGHT_RANGE,
        LIGHT_LIFETIME,
        0.4,
        *shadows != MuzzleShadows::Off,
    );
}

/// Dress an MG shot (slice B): a small 1–2-frame flash (core + one near-only flame plane), a dim
/// short muzzle light on TRACER rounds only, and one faint puff every [`MG_SMOKE_EVERY`] rounds.
/// Everything per-shot randomized (flame frame, roll, size) — at 750 rpm identical repeated
/// flashes strobe. Main-gun-calibre rounds pass through untouched (their dressing is
/// [`on_main_gun_fire`]); staleness and distance LOD gates are the 88's exactly.
fn on_mg_fire(
    fire: On<FireShell>,
    assets: Res<MuzzleVfxAssets>,
    mut materials: ResMut<Assets<VfxBillboardMaterial>>,
    mut ring: ResMut<BillboardRing>,
    mut light_ring: ResMut<MuzzleLightRing>,
    mut cadence: ResMut<MgSmokeCadence>,
    mut shadow_cadence: ResMut<MgShadowCadence>,
    shadows: Res<MuzzleShadows>,
    mut rng: ResMut<ViewRng>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut commands: Commands,
) {
    // The complement of the 88 observer's gate — the SAME boundary the shell-scene/tracer split
    // uses, so every round is dressed by exactly one of the two observers.
    if fire.caliber >= TRACER_MAX_CALIBER {
        return;
    }
    // Stale remote burst (net catch-up past ~250 ms): skip, don't play late.
    if fire.catch_up_ticks > STALE_FIRE_TICKS {
        return;
    }
    // The smoke ration counts every non-stale MG round, near or far, so the cadence is a property
    // of the burst, not of the camera.
    cadence.0 = cadence.0.wrapping_add(1);
    let smoke_due = cadence.0.is_multiple_of(MG_SMOKE_EVERY);

    let origin = fire.origin;
    let dir = Vec3::from(fire.direction);
    // Distance LOD, same shape as the 88's: with no camera (headless harness) treat as near.
    let near = camera
        .single()
        .map(|cam| {
            cam.translation().distance_squared(origin) < FAR_FULL_DRESSING * FAR_FULL_DRESSING
        })
        .unwrap_or(true);

    // --- Flash core: one small camera-facing additive billboard on the MG's own round-glow sprite.
    // The core sprite is single-frame (full-image lanes below — the glow is the WHOLE image, not an
    // atlas cell), so its per-shot variation is roll + size jitter; the flame plane carries the
    // frame variation.
    let mut core = assets.flash_material(assets.mg_core.clone(), 2.0);
    core.params.frame = Vec4::new(0.0, 1.0, 1.0, 0.0);
    let core_size = rng.range(MG_FLASH_CORE_SIZE.0, MG_FLASH_CORE_SIZE.1);
    spawn_billboard(
        &mut commands,
        &mut materials,
        &mut ring,
        assets.quad.clone(),
        BillboardSpec {
            material: core,
            lifetime: MG_FLASH_LIFETIME,
            origin: origin + dir * 0.15,
            drift: Vec3::ZERO,
            frames: 1,
            start_frame: 0.0,
            frame_rate: 0.0,
            start_size: core_size,
            end_size: core_size * 1.2,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: 0.0,
            erosion_end: 0.0,
            rotation: None,
        },
    );

    // --- One bore-aligned flame plane, near only: a random frame of the 4-flame atlas per shot
    // plus a random roll around the bore (survey trick 2 — the anti-strobe variation).
    if near {
        let length = rng.range(MG_FLASH_PLANE_LENGTH.0, MG_FLASH_PLANE_LENGTH.1);
        let rotation = Quat::from_axis_angle(dir, rng.range(0.0, std::f32::consts::TAU))
            * Quat::from_rotation_arc(Vec3::Y, dir);
        spawn_billboard(
            &mut commands,
            &mut materials,
            &mut ring,
            assets.quad.clone(),
            BillboardSpec {
                material: assets.flash_material(assets.flame_atlas.clone(), 2.0),
                lifetime: MG_FLASH_LIFETIME,
                origin: origin + dir * (length * 0.45),
                drift: Vec3::ZERO,
                frames: 4,
                start_frame: rng.range(0.0, 4.0).floor(),
                frame_rate: 0.0,
                start_size: length,
                end_size: length * 1.1,
                aspect: Vec3::new(FLASH_PLANE_WIDTH_RATIO, 1.0, 1.0),
                roll: 0.0,
                spin: 0.0,
                erosion_end: 0.0,
                rotation: Some(rotation),
            },
        );
    }

    // --- Rationed smoke: one faint short puff every few rounds, near only (a sub-pixel puff at
    // range is pure overdraw).
    if near && smoke_due {
        let mut smoke = assets.smoke_material();
        smoke.params.fade.w = MG_SMOKE_ALPHA;
        spawn_billboard(
            &mut commands,
            &mut materials,
            &mut ring,
            assets.quad.clone(),
            BillboardSpec {
                material: smoke,
                lifetime: MG_SMOKE_LIFETIME,
                origin: origin + dir * 0.4,
                drift: Vec3::Y * MG_SMOKE_RISE + dir * MG_SMOKE_PUSH,
                frames: 4,
                start_frame: rng.range(0.0, 4.0),
                frame_rate: SMOKE_FRAME_RATE,
                start_size: MG_SMOKE_SIZE.0,
                end_size: MG_SMOKE_SIZE.1,
                aspect: Vec3::ONE,
                roll: rng.range(0.0, std::f32::consts::TAU),
                spin: rng.range(-SMOKE_SPIN_MAX, SMOKE_SPIN_MAX),
                erosion_end: 1.0,
                rotation: None,
            },
        );
    }

    // --- Muzzle light: EVERY round now (the tracer-only gate is gone — a per-round glimmer reads as
    // real automatic fire). A tracer round spikes brighter so the streak still pops as it leaves the
    // barrel. Shadow casting is the lever's call: On casts always, MgEveryNth casts every
    // [`MG_SHADOW_EVERY`]-th round (the 88 unaffected), Off never.
    shadow_cadence.0 = shadow_cadence.0.wrapping_add(1);
    let mg_shadows = match *shadows {
        MuzzleShadows::On => true,
        MuzzleShadows::Off => false,
        MuzzleShadows::MgEveryNth => shadow_cadence.0.is_multiple_of(MG_SHADOW_EVERY),
    };
    let peak = if fire.tracer {
        MG_LIGHT_PEAK_LUMENS * MG_TRACER_LIGHT_BOOST
    } else {
        MG_LIGHT_PEAK_LUMENS
    };
    spawn_muzzle_light(
        &mut commands,
        &mut light_ring,
        origin + dir * 0.3,
        peak,
        MG_LIGHT_RANGE,
        MG_LIGHT_LIFETIME,
        0.1,
        mg_shadows,
    );
}

/// Decay each muzzle light hard (cubic — most of the drop in the first frames) and despawn at its
/// own lifetime. One system for every gun's lights; the scale rides on the component.
fn decay_muzzle_lights(
    time: Res<Time>,
    mut lights: Query<(Entity, &mut MuzzleLight, &mut PointLight)>,
    mut commands: Commands,
) {
    for (entity, mut light, mut point) in &mut lights {
        light.age += time.delta_secs();
        let t = light.age / light.lifetime;
        if t >= 1.0 {
            // The ring and expiry are independent cleanup owners. Either may already have removed
            // the entity, so cleanup is intentionally idempotent.
            commands.entity(entity).try_despawn();
            continue;
        }
        let falloff = 1.0 - t;
        point.intensity = light.peak * falloff * falloff * falloff;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ballistics::FireShellOrigin;
    use crate::vfx::billboard::Billboard;

    /// Minimal app carrying what BOTH fire observers + the agers read: bare asset stores, a
    /// fixed-seed view RNG, no camera (distance LOD treats that as near — full dressing). Defaults
    /// to the shipped `MuzzleShadows::On`; `harness_shadows` overrides for the lever tests.
    fn harness() -> App {
        harness_shadows(MuzzleShadows::On)
    }

    fn harness_shadows(mode: MuzzleShadows) -> App {
        let mut app = App::new();
        app.init_resource::<BillboardRing>()
            .init_resource::<MuzzleLightRing>()
            .init_resource::<MgSmokeCadence>()
            .init_resource::<MgShadowCadence>()
            .insert_resource(mode)
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<VfxBillboardMaterial>>()
            .init_resource::<Time>()
            .insert_resource(ViewRng::seeded(42))
            .add_observer(on_main_gun_fire)
            .add_observer(on_mg_fire)
            .add_systems(Update, decay_muzzle_lights);
        app.insert_resource(MuzzleVfxAssets {
            quad: Handle::default(),
            core_atlas: Handle::default(),
            mg_core: Handle::default(),
            flame_atlas: Handle::default(),
            smoke_atlas: Handle::default(),
            flash_lut: Handle::default(),
            smoke_lut: Handle::default(),
        });
        app
    }

    fn fire_round(app: &mut App, caliber: f32, catch_up_ticks: u32, tracer: bool) {
        app.world_mut().trigger(FireShell {
            origin: Vec3::new(1.0, 2.0, 3.0),
            direction: Dir3::X,
            speed: 773.0,
            caliber,
            mass: 10.2,
            mechanism: if caliber <= MG_CALIBER {
                crate::spec::FireMechanism::Automatic
            } else {
                crate::spec::FireMechanism::Single
            },
            shooter: None,
            tracer,
            shot_origin: FireShellOrigin::Local,
            catch_up_ticks,
            shot: None,
        });
        app.world_mut().flush();
    }

    fn fire(app: &mut App, caliber: f32, catch_up_ticks: u32) {
        fire_round(app, caliber, catch_up_ticks, true);
    }

    /// The 7.9 mm coax — the MG-calibre side of the boundary.
    const MG_CALIBER: f32 = 0.0079;

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

    /// An 88 shot spawns the full main-gun dressing — core + glow card + 2 planes + smoke
    /// (5 billboards) and 1 light — and an MG-calibre round gets the MG dressing instead: core +
    /// 1 flame plane (no smoke on the first round — the ration counts from 1) at a fraction of the
    /// 88's size, plus its own dim light (now on EVERY round). Each round is dressed by exactly ONE
    /// observer.
    #[test]
    fn main_gun_and_mg_split_the_dressing() {
        let mut app = harness();
        fire(&mut app, 0.088, 0);
        assert_eq!(
            billboards(&mut app),
            5,
            "88: core + glow card + 2 planes + smoke"
        );
        assert_eq!(lights(&mut app), 1);

        let mut mg = harness();
        fire_round(&mut mg, MG_CALIBER, 0, true);
        assert_eq!(billboards(&mut mg), 2, "MG: core + 1 flame plane");
        assert_eq!(lights(&mut mg), 1, "every MG round carries a light");
        // Scale discipline: every MG flash element is well under the 88's smallest core.
        let world = mg.world_mut();
        let mut q = world.query::<&Billboard>();
        for billboard in q.iter(world) {
            assert!(
                billboard.start_size < FLASH_CORE_SIZE.0,
                "MG dressing must stay rifle-scale (got {} m)",
                billboard.start_size
            );
        }
    }

    /// Distance LOD contract: beyond `FAR_FULL_DRESSING` only the core + light spawn — the glow
    /// card, flame planes and smoke are all near-only. A camera parked well past the cutoff must
    /// leave the 88 with exactly ONE billboard (the core) and its light. Guards the glow card
    /// against silently re-escaping the gate.
    #[test]
    fn far_88_shot_drops_to_core_and_light() {
        let mut app = harness();
        // Park a camera far past the 400 m cutoff from the fixed shot origin (1, 2, 3).
        app.world_mut().spawn((
            Camera3d::default(),
            GlobalTransform::from_translation(Vec3::new(2000.0, 0.0, 0.0)),
        ));
        fire(&mut app, 0.088, 0);
        assert_eq!(
            billboards(&mut app),
            1,
            "far 88: core only (no glow card, planes, or smoke)"
        );
        assert_eq!(
            lights(&mut app),
            1,
            "the muzzle light carries the read at range"
        );
    }

    /// Per-shot variation is the MG's anti-strobe contract: consecutive shots must differ in core
    /// roll and size (seeded RNG makes this deterministic — a regression to fixed values fails).
    #[test]
    fn mg_shots_never_repeat_identically() {
        let mut app = harness();
        fire_round(&mut app, MG_CALIBER, 0, false);
        fire_round(&mut app, MG_CALIBER, 0, false);
        let world = app.world_mut();
        // The cores are the camera-facing billboards (the flame planes bake a fixed rotation).
        let mut q = world.query_filtered::<&Billboard, With<crate::vfx::billboard::FaceCamera>>();
        let cores: Vec<(f32, f32)> = q.iter(world).map(|b| (b.roll, b.start_size)).collect();
        assert_eq!(cores.len(), 2, "two shots, two cores");
        assert!(
            cores[0].0 != cores[1].0 && cores[0].1 != cores[1].1,
            "consecutive MG flashes must differ in roll and size: {cores:?}"
        );
    }

    /// The MG muzzle light now rides EVERY round (the tracer-only gate is gone): a 4-ball-1-tracer
    /// belt cycle yields five lights, each dimmer and shorter-lived than the 88's, and the tracer
    /// round's light spikes [`MG_TRACER_LIGHT_BOOST`]× brighter than a ball round's.
    #[test]
    fn mg_light_rides_every_round_with_tracer_spike() {
        let mut app = harness();
        for _ in 0..4 {
            fire_round(&mut app, MG_CALIBER, 0, false);
        }
        assert_eq!(lights(&mut app), 4, "every ball round carries a light too");
        fire_round(&mut app, MG_CALIBER, 0, true);
        assert_eq!(lights(&mut app), 5, "the tracer round adds its own");

        // Scale + spike: a fresh app so exactly one ball then one tracer are comparable at birth.
        let mut ball = harness();
        fire_round(&mut ball, MG_CALIBER, 0, false);
        let ball_peak = {
            let world = ball.world_mut();
            let mut q = world.query::<&MuzzleLight>();
            q.single(world).expect("one ball light").peak
        };
        let mut tracer = harness();
        fire_round(&mut tracer, MG_CALIBER, 0, true);
        let world = tracer.world_mut();
        let mut q = world.query::<(&MuzzleLight, &PointLight)>();
        let (light, point) = q.single(world).expect("one tracer light");
        assert!(light.peak < LIGHT_PEAK_LUMENS, "dimmer than the 88's");
        assert!(light.lifetime < LIGHT_LIFETIME, "shorter than the 88's");
        assert!(
            (light.peak - ball_peak * MG_TRACER_LIGHT_BOOST).abs() < 1.0,
            "the tracer round's light spikes {MG_TRACER_LIGHT_BOOST}× the ball round's"
        );
        // Under the default (shadows On) even the MG light casts.
        assert!(
            point.shadow_maps_enabled,
            "shadows On casts on the MG light"
        );
    }

    /// The shadow lever: `On` casts on both guns, `Off` casts on neither, `MgEveryNth` spares the MG
    /// except every [`MG_SHADOW_EVERY`]-th round while the 88 always casts.
    #[test]
    fn shadow_lever_gates_casting() {
        // Off: neither gun casts.
        let mut off = harness_shadows(MuzzleShadows::Off);
        fire(&mut off, 0.088, 0);
        fire_round(&mut off, MG_CALIBER, 0, true);
        {
            let world = off.world_mut();
            let mut q = world.query::<&PointLight>();
            for point in q.iter(world) {
                assert!(!point.shadow_maps_enabled, "Off: no light casts");
            }
        }

        // MgEveryNth: the 88 casts; the MG casts only on the Nth round.
        let mut nth = harness_shadows(MuzzleShadows::MgEveryNth);
        fire(&mut nth, 0.088, 0);
        {
            let world = nth.world_mut();
            let mut q = world.query::<(&MuzzleLight, &PointLight)>();
            let (_, point) = q.single(world).expect("just the 88 light");
            assert!(point.shadow_maps_enabled, "MgEveryNth: the 88 still casts");
        }
        // Walk one belt cycle of MG rounds: exactly one — the Nth — casts a shadow.
        let mut walk = harness_shadows(MuzzleShadows::MgEveryNth);
        for _ in 0..MG_SHADOW_EVERY {
            fire_round(&mut walk, MG_CALIBER, 0, false);
        }
        let world = walk.world_mut();
        let mut q = world.query::<&PointLight>();
        let casters = q.iter(world).filter(|p| p.shadow_maps_enabled).count();
        assert_eq!(
            casters, 1,
            "MgEveryNth: exactly the {MG_SHADOW_EVERY}-th of a belt cycle casts"
        );
    }

    /// MG smoke is rationed to every [`MG_SMOKE_EVERY`]-th round — per-round puffs at the cyclic
    /// rate are the overdraw trap the survey warns about.
    #[test]
    fn mg_smoke_spawns_every_nth_round() {
        let mut app = harness();
        let rounds = MG_SMOKE_EVERY * 2;
        for _ in 0..rounds {
            fire_round(&mut app, MG_CALIBER, 0, false);
        }
        // Each round spawns core + flame plane; every Nth adds one puff.
        let expected = (rounds * 2 + rounds / MG_SMOKE_EVERY) as usize;
        assert_eq!(
            billboards(&mut app),
            expected,
            "2/round + 1 puff per {MG_SMOKE_EVERY}"
        );
    }

    /// A stale remote shot (catch-up beyond ~250 ms) skips the dressing rather than playing late —
    /// both guns.
    #[test]
    fn stale_remote_fire_skips_dressing() {
        let mut app = harness();
        fire(&mut app, 0.088, STALE_FIRE_TICKS + 1);
        assert_eq!(billboards(&mut app), 0);
        assert_eq!(lights(&mut app), 0);
        fire(&mut app, MG_CALIBER, STALE_FIRE_TICKS + 1);
        assert_eq!(billboards(&mut app), 0, "stale MG burst skips too");
        assert_eq!(lights(&mut app), 0);
        // At or under the boundary the dressing still plays (~150 ms catch-up is the normal case).
        fire(&mut app, 0.088, STALE_FIRE_TICKS);
        assert_eq!(billboards(&mut app), 5);
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
            point.shadow_maps_enabled,
            "under the default shadows-On lever the 88 light casts (the 2026-07-12 decision)"
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
        let mut spawned = Vec::with_capacity(LIGHT_CAP + 5);
        for _ in 0..LIGHT_CAP + 5 {
            fire(&mut app, 0.088, 0);
            spawned.push(
                *app.world()
                    .resource::<MuzzleLightRing>()
                    .0
                    .back()
                    .expect("each round enters the light ring"),
            );
        }
        assert_eq!(lights(&mut app), LIGHT_CAP);
        for entity in &spawned[..5] {
            assert!(
                app.world().get::<MuzzleLight>(*entity).is_none(),
                "oldest lights are evicted first",
            );
        }
        for entity in &spawned[5..] {
            assert!(
                app.world().get::<MuzzleLight>(*entity).is_some(),
                "the newest capped window survives",
            );
        }
    }
}
