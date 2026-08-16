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

/// Flash core lifetime in seconds. The 88's hot layers are STAGGERED — core dies first, then the
/// flame planes ([`FLASH_PLANE_LIFETIME`]), then the glow card ([`FLASH_GLOW_CARD_LIFETIME`]), then
/// the gas smoke ([`SMOKE_LIFETIME`]) — and every one of them is at maximum size on frame one
/// (`end_size` below `start_size`; nothing in the flash cluster grows in).
const FLASH_LIFETIME: f32 = 0.026;
/// Flame-plane lifetime (s): outlives the core, dead well before the glow card.
const FLASH_PLANE_LIFETIME: f32 = 0.045;
/// Core flash diameter range in metres.
const FLASH_CORE_SIZE: (f32, f32) = (3.5, 4.6);
/// Shrink factors the hot layers ease to over their life (< 1: born maximal, collapsing).
const FLASH_CORE_SHRINK: f32 = 0.85;
const FLASH_PLANE_SHRINK: f32 = 0.9;
const FLASH_GLOW_CARD_SHRINK: f32 = 0.9;
/// Directional flame-plane length range in metres and width ratio.
const FLASH_PLANE_LENGTH: (f32, f32) = (4.3, 6.4);
const FLASH_PLANE_WIDTH_RATIO: f32 = 0.55;
/// Emissive boost on the flash LUT's heat lane — well above 1.0 so bloom catches the whole flash.
const FLASH_GLOW: f32 = 14.0;

// --- The 88's BLAST CORE: a real 12-frame explosion flipbook, played once, that carries the
// fireball the whole muzzle moment is built around. Every metric is authored at [`BLAST_CALIBER`]
// and scales LINEARLY with the firing bore (ADR-0023). The flash core above stays the frame-one
// blinding spike inside it; the flame planes stay the directional jets.

/// Atlas grid and frame count — [`MuzzleVfxAssets::blast_atlas`]'s two sheets share it
/// (`scripts/vfx/blast_atlas.py`).
const BLAST_COLS: f32 = 4.0;
const BLAST_ROWS: f32 = 3.0;
const BLAST_FRAMES: u32 = 12;
/// The bore (m) the metres and seconds below are authored against; the blast scales `caliber / this`.
const BLAST_CALIBER: f32 = 0.088;
/// Quad diameter range (m). The sprite's fireball spans ~a third of its cell on frame one and ~three
/// quarters by the last, so the rendered ball runs ~3 m at ignition through ~5 m at its hottest.
const BLAST_SIZE: (f32, f32) = (8.0, 9.5);
/// Quad growth over life: the gas keeps pushing outward past the sprite's own expansion.
const BLAST_GROWTH: f32 = 1.12;
/// Standoff down the bore (m) of the fireball's centre from the muzzle.
const BLAST_STANDOFF: f32 = 1.8;
/// Billboard lifetime (s), and the seconds the [`BLAST_FRAMES`] frames would take at the rate
/// derived from it. The playthrough is deliberately the LONGER of the two: `flipbook_frame` wraps
/// modulo the frame count, so the sequence has to still be inside its last cell when
/// `age_billboards` despawns the quad — a playthrough at or under the lifetime hands back frame 0
/// for the closing instant, and the fireball re-ignites as it dies.
const BLAST_LIFETIME: f32 = 0.2;
const BLAST_PLAYTHROUGH: f32 = 0.205;
/// Overall alpha, erosion sharpness and emissive boost. The boost sits under [`FLASH_GLOW`] — a
/// fireball with volume, not a second blinding spike — and the soft sharpness keeps the smoke fringe
/// as fringe.
const BLAST_ALPHA: f32 = 0.85;
const BLAST_SHARPNESS: f32 = 1.3;
const BLAST_GLOW: f32 = 6.0;
/// Erosion threshold at death. Low, unlike every other muzzle layer: the sheet ALREADY animates its
/// own dissolution, and the die-off is the blast LUT's cooling term (which reaches black at the last
/// LUT row, so the layer is gone before it despawns). Erosion here only thins the fringe — ramped to
/// 1.0 it instead eats the fireball into confetti through the middle of the sequence.
const BLAST_EROSION: f32 = 0.3;

/// The 88's fireball glow card: ONE soft additive billboard behind the starburst core — the classic
/// card that sells fireball VOLUME between the 2-frame flash and the lingering smoke. Camera-facing,
/// ~1.5× the core, LOW alpha, on the round-glow (`mg_core`) sprite, and it erodes out over its own
/// lifetime — the LAST beat of the flash cluster, still an order of magnitude under the smoke.
/// Rides the same additive billboard pipeline as the flash (no new permutation, so the prewarm rig
/// already covers it).
const FLASH_GLOW_CARD_SCALE: f32 = 1.5;
const FLASH_GLOW_CARD_LIFETIME: f32 = 0.075;
/// LOW overall alpha (billboard `fade.w`) and a softened emissive boost (`glow.x`) — a fill glow, not
/// a second hot core.
const FLASH_GLOW_CARD_ALPHA: f32 = 0.35;
const FLASH_GLOW_CARD_GLOW: f32 = 4.0;

/// Lingering muzzle smoke: lifetime (s), size ease (m), and its drift (up + a muzzle-gas push).
/// The one 88 layer that grows IN — birth is ~half the end size, and it outlives the whole flash
/// cluster by an order of magnitude.
const SMOKE_LIFETIME: f32 = 1.5;
const SMOKE_SIZE: (f32, f32) = (2.2, 4.2);
const SMOKE_RISE: f32 = 0.55;
const SMOKE_PUSH: f32 = 1.3;
/// Slow flipbook playback for the smoke (frames/s over the 4-frame atlas) and its roll rate (rad/s).
const SMOKE_FRAME_RATE: f32 = 5.0;
const SMOKE_SPIN_MAX: f32 = 0.6;
/// Faint heat on young smoke (it is lit by the flash for the first instants).
const SMOKE_GLOW: f32 = 5.0;

/// Main-gun light peak (lm), range (m), and lifetime (s) — a flash-and-out envelope: cubic decay
/// off the first-frame peak, dark inside ~55 ms. Stays strictly longer than the MG glimmer's
/// ([`MG_LIGHT_LIFETIME`]).
const LIGHT_PEAK_LUMENS: f32 = 8.0e6;
const LIGHT_RANGE: f32 = 35.0;
const LIGHT_LIFETIME: f32 = 0.055;
/// Shared muzzle-light population cap; oldest lights are evicted first.
const LIGHT_CAP: usize = 12;
/// The MG tracer-round brightness spike: a tracer round's muzzle light is this much brighter than a
/// ball round's, so the flicker still reads harder exactly when a streak leaves the barrel.
const MG_TRACER_LIGHT_BOOST: f32 = 1.5;

// --- The muzzle GROUND DUST cloud (main gun only): the blast wave lifting the earth under the
// barrel. Gated on the muzzle sitting within [`GROUND_DUST_MAX_HEIGHT`] of the terrain below it
// ([`crate::terrain_grid::HeightGrid::height_at`] — no grid resource means no known ground and no
// cloud). Every metric below is authored at [`GROUND_DUST_CALIBER`] and scales LINEARLY with the
// firing bore (ADR-0023: physical inputs only — the camera never enters this).

/// Muzzle height above the ground under it (m) at which the blast still lifts dust.
const GROUND_DUST_MAX_HEIGHT: f32 = 2.5;
/// The bore (m) the sizes/speeds below are authored against; the cloud scales `caliber / this`.
const GROUND_DUST_CALIBER: f32 = 0.088;
/// Puffs per shot, inclusive range.
const GROUND_DUST_COUNT: (u32, u32) = (5, 8);
/// Puff size ease (m): birth diameter → end of life (the cloud billows as it spreads).
const GROUND_DUST_SIZE: (f32, f32) = (1.8, 5.0);
/// Birth-offset range (m) from the ground point under the muzzle, along each puff's own azimuth.
const GROUND_DUST_SPREAD: (f32, f32) = (0.4, 3.0);
/// Outward drift speed range (m/s) and the slow lift that keeps the cloud hugging the ground; both
/// scale with the bore like every other metric here.
const GROUND_DUST_PUSH: (f32, f32) = (1.8, 4.0);
const GROUND_DUST_RISE: f32 = 0.35;
/// How far each puff's random azimuth is pulled onto the bore azimuth (0 = radial ring, 1 = all
/// straight down-bore).
const GROUND_DUST_BORE_BIAS: f32 = 0.65;
/// Cloud lifetime range (s) — the slowest muzzle layer, well past the gas smoke.
const GROUND_DUST_LIFETIME: (f32, f32) = (2.0, 2.5);
/// Overall alpha of a puff, its flipbook rate (frames/s over the 4-frame dust atlas), and its roll
/// rate bound (rad/s).
const GROUND_DUST_ALPHA: f32 = 0.5;
const GROUND_DUST_FRAME_RATE: f32 = 3.0;
const GROUND_DUST_SPIN_MAX: f32 = 0.35;

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
    let shadows = MuzzleShadows::from_env();
    // The RESOLVED policy, logged verbatim as the token the env knob accepts — an A/B runner
    // (`scripts/perf/run-fire-capture.sh`) greps this per condition and fails on a mismatch, so a
    // typo'd or dropped `OVERMATCH_MUZZLE_SHADOWS` cannot silently measure the default arm twice.
    info!("muzzle_shadows: resolved {}", shadows.token());
    app.init_resource::<MuzzleLightRing>()
        .init_resource::<MgSmokeCadence>()
        .insert_resource(shadows)
        .add_systems(Startup, setup_muzzle_assets)
        .add_observer(on_main_gun_fire)
        .add_observer(on_mg_fire)
        .add_systems(Update, decay_muzzle_lights);
}

/// Muzzle-light shadow policy, read once at plugin setup.
///
/// The default is [`Self::MainGunOnly`] on MEASURED evidence (M4, 2 tanks, `M350` shadows, vsync
/// off, `scripts/perf/run-fire-capture.sh` 2026-07-31): a shadow-casting point light per MG round
/// at the Tiger's 2 × 750 rpm is the single dominant cost of holding the trigger. Idle blew the
/// 60 fps budget on 3.6% of frames; with both MGs firing that became ~42% — while the same scene
/// with muzzle shadows off moved only 14.4% → 16.8%. p50 tracked it: 13.5 → 15.5 ms firing with
/// MG shadow casting, 13.5 → 14.6 ms without.
///
/// The mechanism is that a point light's shadow is SIX cubemap faces, each re-submitting the
/// shooter's own tank — and a Tiger is the project's known worst shadow caster (194 track links
/// per pass). At ~25 rounds/s over a 0.05 s light lifetime that is a shadow cubemap being built,
/// used for ~3 frames, and thrown away, continuously, for a 16 m glimmer that lasts long enough to
/// be seen but nowhere near long enough for its SHADOW to be read.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum MuzzleShadows {
    /// The shipped decision: the 88's flash casts (one shot per ~3 s, a real lighting event), the
    /// MGs' do not.
    #[default]
    MainGunOnly,
    /// Every muzzle light casts, MG rounds included — what shipped before the measurement above,
    /// kept as that A/B's arm rather than deleted, since it is the only way to re-run it.
    On,
    /// No muzzle light casts a shadow (the measurement baseline / hard fallback).
    Off,
}

impl MuzzleShadows {
    /// Parse `OVERMATCH_MUZZLE_SHADOWS`. Every variant has an EXPLICIT token, the shipped default
    /// included: an A/B arm that wants the shipped policy has to be able to NAME it, or its
    /// "shipped" condition silently becomes whatever the default happens to be on the day — which
    /// is exactly how this script's `shipped` arm came to export the legacy policy after the
    /// default moved to [`Self::MainGunOnly`].
    fn from_env() -> Self {
        Self::parse(std::env::var("OVERMATCH_MUZZLE_SHADOWS").ok().as_deref())
    }

    /// The knob's whole grammar, split out so it is testable without writing process-global state.
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("off") => Self::Off,
            Some("on") => Self::On,
            Some("main-only") => Self::MainGunOnly,
            // Unset or anything else: the default decision. Unrecognized values are visible in the
            // resolved-policy log line, which is what the runner verifies against.
            _ => Self::MainGunOnly,
        }
    }

    /// The token this policy is named by — the same string [`Self::from_env`] accepts, so the
    /// logged value round-trips back through the knob.
    fn token(self) -> &'static str {
        match self {
            Self::MainGunOnly => "main-only",
            Self::On => "on",
            Self::Off => "off",
        }
    }

    /// Whether an MG round's muzzle light casts — the hot path this lever exists for.
    fn mg_casts(self) -> bool {
        self == Self::On
    }

    /// Whether the main gun's muzzle light casts.
    fn main_gun_casts(self) -> bool {
        self != Self::Off
    }
}

/// Preloaded muzzle-dressing assets: the shared quad, the two sprite atlases, and the per-effect
/// gradient LUTs (one grayscale sprite set, recolored per effect — the LUT trick).
#[derive(Resource)]
pub(super) struct MuzzleVfxAssets {
    pub(super) quad: Handle<Mesh>,
    /// The 88's flash core: a 2×2 scorch-starburst atlas (a spiky radial star per shot).
    pub(super) core_atlas: Handle<Image>,
    /// The 88's blast core: two 4×3 explosion flipbooks, one picked per shot. Two sheets rather
    /// than one is the anti-repetition trick at sprite scale — the sequence inside a sheet is
    /// fixed (it starts on its most violent frame and plays once), so the variation has to come
    /// from WHICH fireball plays.
    blast_atlas: [Handle<Image>; 2],
    /// The MG's flash core: a single small round glow (`light_01`) — a rifle-scale pop, not the 88's
    /// starburst.
    mg_core: Handle<Image>,
    flame_atlas: Handle<Image>,
    smoke_atlas: Handle<Image>,
    /// The billow atlas the impact read also loads (the asset server dedupes the load and shares
    /// the GPU texture) — the mass sprite the ground-dust cloud is drawn from.
    dust_atlas: Handle<Image>,
    flash_lut: Handle<Image>,
    /// The blast core's own palette — the flash LUT drives nearly the whole signal band to
    /// white-hot, which is the read a 2-frame starburst wants and the wrong one for a 12-frame
    /// sheet whose MID-tones are the fire.
    blast_lut: Handle<Image>,
    smoke_lut: Handle<Image>,
    /// Earth palette for the ground-dust cloud — brown lifted soil, never the smoke gray.
    dust_lut: Handle<Image>,
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

    /// The blast-core material template: one of the two flipbook sheets through the blast LUT.
    /// Rides the flash's additive pipeline (no new permutation), with its own 4×3 grid lanes,
    /// softer erosion and a lower boost.
    fn blast_material(&self, atlas: Handle<Image>) -> VfxBillboardMaterial {
        let mut material = self.flash_material(atlas, BLAST_SHARPNESS);
        material.params.frame = Vec4::new(0.0, BLAST_COLS, BLAST_ROWS, 0.0);
        material.params.fade.w = BLAST_ALPHA;
        material.params.glow.x = BLAST_GLOW;
        material.lut = self.blast_lut.clone();
        material
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

    /// The ground-dust material template: the billow atlas through the earth LUT, alpha-blend
    /// occluding mass with no heat (lifted soil is lit mass, never an emitter). Rides the smoke's
    /// already-warmed Blend pipeline.
    fn dust_material(&self) -> VfxBillboardMaterial {
        VfxBillboardMaterial {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, 2.2, 0.0, GROUND_DUST_ALPHA),
                glow: Vec4::new(0.0, 0.0, 0.0, 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: self.dust_lut.clone(),
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
    // Blast LUT: a fire ramp rather than the flash's white-out — black through deep red into
    // orange, white-hot only in the top of the signal band. Heat rides the white-hot fraction, so
    // only the core blooms while the fireball's orange body stays readable as fire. The life axis
    // is this layer's whole die-off: `cool` holds through the fireball's first half and reaches
    // ZERO at the last LUT row, so the blast is dark before its quad despawns.
    let blast_lut = gradient_lut(&mut images, |x, y| {
        let floor = smoothstep(0.04, 0.28, x);
        let hot = smoothstep(0.35, 1.0, x);
        let cool = 1.0 - smoothstep(0.25, 1.0, y);
        let lum = (0.18 + 0.82 * x) * floor;
        let color = LinearRgba::rgb(
            lum,
            lum * (0.18 + 0.72 * hot),
            lum * (0.03 + 0.60 * hot * hot),
        ) * cool;
        (color, hot * cool)
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
    // Dust LUT: sunlit tan where the sprite signal is strong, sinking to a dark damp brown in the
    // cloud's shadow and darkening as it ages (Y). No heat lane — lifted soil never emits.
    let dust_lut = gradient_lut(&mut images, |x, y| {
        let lum = 0.07 + 0.36 * x;
        let age = 1.0 - y;
        let color = LinearRgba::rgb(
            lum * (0.80 + 0.28 * age),
            lum * (0.61 + 0.19 * age),
            lum * (0.43 + 0.11 * age),
        );
        (color, 0.0)
    });
    commands.insert_resource(MuzzleVfxAssets {
        quad: unit_quad(&mut meshes),
        core_atlas: asset_server.load("vfx/flash_core_atlas.png"),
        blast_atlas: [
            asset_server.load("vfx/blast_core_a.png"),
            asset_server.load("vfx/blast_core_b.png"),
        ],
        mg_core: asset_server.load("vfx/mg_core.png"),
        flame_atlas: asset_server.load("vfx/flash_flames_atlas.png"),
        smoke_atlas: asset_server.load("vfx/smoke_atlas.png"),
        dust_atlas: asset_server.load("vfx/impact_dust.png"),
        flash_lut,
        blast_lut,
        smoke_lut,
        dust_lut,
    });
}

/// A live muzzle light's age plus the scale it was born with; [`decay_muzzle_lights`] drives the
/// intensity fall and the despawn from these. Per-light `peak`/`lifetime` is what lets the 88 and
/// the MGs share one decay system at different scales.
#[derive(Component)]
struct MuzzleLight {
    age: f32,
    /// Cleared until [`decay_muzzle_lights`]'s first visit, which only arms it: the light's first
    /// rendered frame is the spawn-time peak, and `age` starts advancing the frame after.
    armed: bool,
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
                armed: false,
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
            // "The hull occludes it like any object" is only true while the light can SEE the hull.
            // This light does cast by default, and with the vendored shadow-view patch its shadow
            // view inherits this mask — so without a profile it would neither light nor be occluded
            // by the player's own tank (which lives on its own channel), and the track ribbon would
            // stop casting from it too.
            crate::render_policy::LightProfile::BattlefieldMuzzleFlash,
            Transform::from_translation(position),
        ))
        .id();
    crate::push_capped_entity(commands, &mut ring.0, light, LIGHT_CAP);
}

/// Dress a main-gun shot: blast core + flash cluster + muzzle light + lingering smoke + the
/// ground-dust cloud a low barrel lifts, all view entities hung off the `FireShell` geometry
/// (origin + bore direction).
/// MG-calibre rounds pass through untouched — their dressing is slice B, on this same machinery.
fn on_main_gun_fire(
    fire: On<FireShell>,
    assets: Res<MuzzleVfxAssets>,
    mut materials: ResMut<Assets<VfxBillboardMaterial>>,
    mut ring: ResMut<BillboardRing>,
    mut light_ring: ResMut<MuzzleLightRing>,
    shadows: Res<MuzzleShadows>,
    mut rng: ResMut<ViewRng>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    // The decoded terrain surface, when the world has one — the ground-dust gate's only query.
    grid: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut commands: Commands,
) {
    // The same boundary as the shell-scene branch in `ballistics::view`: this dressing is
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
            end_size: core_size * FLASH_CORE_SHRINK,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: 0.0,
            erosion_end: 0.0,
            rotation: None,
        },
    );

    // --- Blast core: the fireball flipbook, played ONCE from its most violent frame. Not gated on
    // `near`: it is the shot's primary read at any range, and one quad on the main gun's ~3 s
    // cadence is not the near-field overdraw the LOD gate bounds.
    let bore = fire.caliber / BLAST_CALIBER;
    let blast_size = rng.range(BLAST_SIZE.0, BLAST_SIZE.1) * bore;
    let blast_lifetime = BLAST_LIFETIME * bore;
    spawn_billboard(
        &mut commands,
        &mut materials,
        &mut ring,
        assets.quad.clone(),
        BillboardSpec {
            material: assets.blast_material(assets.blast_atlas[blast_pick(&mut rng)].clone()),
            lifetime: blast_lifetime,
            origin: origin + dir * (BLAST_STANDOFF * bore),
            drift: Vec3::ZERO,
            frames: BLAST_FRAMES,
            // Frame 0 is the ignition frame — no random start offset here: the sheet is a real
            // sequence, and the per-shot variation is which sheet plays.
            start_frame: 0.0,
            frame_rate: BLAST_FRAMES as f32 / (BLAST_PLAYTHROUGH * bore),
            start_size: blast_size,
            end_size: blast_size * BLAST_GROWTH,
            aspect: Vec3::ONE,
            roll: rng.range(0.0, std::f32::consts::TAU),
            spin: 0.0,
            erosion_end: BLAST_EROSION,
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
                end_size: glow_size * FLASH_GLOW_CARD_SHRINK,
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
                    lifetime: FLASH_PLANE_LIFETIME,
                    origin: origin + dir * (length * 0.45),
                    drift: Vec3::ZERO,
                    frames: 4,
                    start_frame: (plane_frame + i as f32) % 4.0,
                    frame_rate: 0.0,
                    start_size: length,
                    end_size: length * FLASH_PLANE_SHRINK,
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

    // --- Ground dust: the blast lifting the earth under a low barrel. Near-only like the rest of
    // the full dressing (the LOD contract is about COST — the cloud's metres come from the bore,
    // never from the camera), and only when the terrain under the muzzle is close enough to lift.
    if near && let Some(ground_y) = dust_ground_y(grid.as_deref(), origin) {
        spawn_ground_dust(
            origin,
            dir,
            ground_y,
            fire.caliber,
            &assets,
            &mut materials,
            &mut ring,
            &mut rng,
            &mut commands,
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
        shadows.main_gun_casts(),
    );
}

/// Which of the two blast sheets this shot plays. A sheet's own sequence is fixed — it opens on
/// its most violent frame and runs once — so consecutive shots are kept apart by the pick, not by
/// the start frame the other layers randomize.
fn blast_pick(rng: &mut ViewRng) -> usize {
    usize::from(rng.next_f32() < 0.5)
}

/// The ground height (m) under `muzzle` when a shot there lifts dust, else `None`: no decoded
/// terrain (`grid`) and no ground is known, and outside the grid's span the world ends at the
/// collider edge — [`crate::terrain_grid::HeightGrid::height_at`] would hand back a clamped
/// phantom there. A muzzle higher than [`GROUND_DUST_MAX_HEIGHT`] over that surface raises nothing.
fn dust_ground_y(grid: Option<&crate::terrain_grid::HeightGrid>, muzzle: Vec3) -> Option<f32> {
    let grid = grid?;
    if !grid.contains_xz(muzzle.x, muzzle.z) {
        return None;
    }
    let ground = grid.height_at(muzzle.x, muzzle.z);
    (muzzle.y - ground <= GROUND_DUST_MAX_HEIGHT).then_some(ground)
}

/// The ground-dust cloud: [`GROUND_DUST_COUNT`] wide alpha-blend puffs born on the ground under the
/// muzzle and pushed outward, each along an azimuth pulled toward the bore's by
/// [`GROUND_DUST_BORE_BIAS`]. Sizes, offsets and push speeds scale linearly with `caliber` against
/// [`GROUND_DUST_CALIBER`]; the puffs ride the smoke's Blend pipeline and the shared billboard ring.
#[allow(clippy::too_many_arguments)]
fn spawn_ground_dust(
    muzzle: Vec3,
    dir: Vec3,
    ground_y: f32,
    caliber: f32,
    assets: &MuzzleVfxAssets,
    materials: &mut Assets<VfxBillboardMaterial>,
    ring: &mut BillboardRing,
    rng: &mut ViewRng,
    commands: &mut Commands,
) {
    let bore = caliber / GROUND_DUST_CALIBER;
    let base = Vec3::new(muzzle.x, ground_y, muzzle.z);
    // The bore's ground-plane azimuth; a vertical barrel has none, and the cloud stays a plain ring.
    let bore_xz = Vec3::new(dir.x, 0.0, dir.z).try_normalize();
    let count = GROUND_DUST_COUNT.0
        + (rng.next_f32() * (GROUND_DUST_COUNT.1 - GROUND_DUST_COUNT.0 + 1) as f32) as u32;
    for _ in 0..count.min(GROUND_DUST_COUNT.1) {
        let theta = rng.range(0.0, std::f32::consts::TAU);
        let radial = Vec3::new(theta.cos(), 0.0, theta.sin());
        let away = match bore_xz {
            Some(bore_dir) => (radial * (1.0 - GROUND_DUST_BORE_BIAS)
                + bore_dir * GROUND_DUST_BORE_BIAS)
                .try_normalize()
                .unwrap_or(bore_dir),
            None => radial,
        };
        let start_size = GROUND_DUST_SIZE.0 * bore;
        let end_size = rng.range(GROUND_DUST_SIZE.1 * 0.75, GROUND_DUST_SIZE.1) * bore;
        let offset = rng.range(GROUND_DUST_SPREAD.0, GROUND_DUST_SPREAD.1) * bore;
        let push = rng.range(GROUND_DUST_PUSH.0, GROUND_DUST_PUSH.1) * bore;
        spawn_billboard(
            commands,
            materials,
            ring,
            assets.quad.clone(),
            BillboardSpec {
                material: assets.dust_material(),
                lifetime: rng.range(GROUND_DUST_LIFETIME.0, GROUND_DUST_LIFETIME.1),
                // Half a birth diameter up: the puff's lower edge sits ON the surface.
                origin: base + away * offset + Vec3::Y * (start_size * 0.5),
                drift: away * push + Vec3::Y * (GROUND_DUST_RISE * bore),
                frames: 4,
                start_frame: rng.range(0.0, 4.0),
                frame_rate: GROUND_DUST_FRAME_RATE,
                start_size,
                end_size,
                aspect: Vec3::ONE,
                roll: rng.range(0.0, std::f32::consts::TAU),
                spin: rng.range(-GROUND_DUST_SPIN_MAX, GROUND_DUST_SPIN_MAX),
                erosion_end: 1.0,
                rotation: None,
            },
        );
    }
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

    // --- Muzzle light: EVERY round (the tracer-only gate is gone — a per-round glimmer reads as
    // real automatic fire). A tracer round spikes brighter so the streak still pops as it leaves the
    // barrel. It does NOT cast a shadow under the shipped lever: six cubemap faces of the shooter's
    // own Tiger, per round, at 750 rpm, was the measured dominant cost of holding the trigger — see
    // [`MuzzleShadows`] for the numbers.
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
        shadows.mg_casts(),
    );
}

/// Decay each muzzle light hard (cubic — most of the drop in the first frames) and despawn at its
/// own lifetime. One system for every gun's lights; the scale rides on the component. A newborn is
/// only ARMED on the first visit, so the frame a light was spawned in renders at its peak whichever
/// side of this system the spawn landed on.
fn decay_muzzle_lights(
    time: Res<Time>,
    mut lights: Query<(Entity, &mut MuzzleLight, &mut PointLight)>,
    mut commands: Commands,
) {
    for (entity, mut light, mut point) in &mut lights {
        if !light.armed {
            light.armed = true;
            continue;
        }
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
    use crate::vfx::billboard::{Billboard, age_billboards};

    /// Minimal app carrying what BOTH fire observers + the agers read: bare asset stores, a
    /// fixed-seed view RNG, no camera (distance LOD treats that as near — full dressing), and NO
    /// height grid (so the ground-dust gate finds no known ground; `with_ground` adds one).
    /// Defaults to the shipped `MuzzleShadows::MainGunOnly`; `harness_shadows` overrides for the
    /// lever tests.
    fn harness() -> App {
        harness_shadows(MuzzleShadows::default())
    }

    fn harness_shadows(mode: MuzzleShadows) -> App {
        let mut app = App::new();
        app.init_resource::<BillboardRing>()
            .init_resource::<MuzzleLightRing>()
            .init_resource::<MgSmokeCadence>()
            .insert_resource(mode)
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<VfxBillboardMaterial>>()
            .init_resource::<Time>()
            .insert_resource(ViewRng::seeded(42))
            .add_observer(on_main_gun_fire)
            .add_observer(on_mg_fire)
            // BOTH agers: the dressing's contract is what a layer RENDERS after n frames, and the
            // billboard half of it is unreadable without `age_billboards` running.
            .add_systems(Update, (age_billboards, decay_muzzle_lights));
        // The two blast sheets get DISTINCT handles (everything else can share the default one):
        // which sheet a shot picked is only readable off its material, so the pick test needs
        // them to be tellable apart.
        let blast_atlas = app
            .world_mut()
            .resource_scope(|_, mut images: Mut<Assets<Image>>| {
                [(); 2].map(|()| images.add(Image::default()))
            });
        app.insert_resource(MuzzleVfxAssets {
            quad: Handle::default(),
            core_atlas: Handle::default(),
            blast_atlas,
            mg_core: Handle::default(),
            flame_atlas: Handle::default(),
            smoke_atlas: Handle::default(),
            dust_atlas: Handle::default(),
            flash_lut: Handle::default(),
            blast_lut: Handle::default(),
            smoke_lut: Handle::default(),
            dust_lut: Handle::default(),
        });
        app
    }

    /// The harness with a FLAT terrain surface at `height` under it — what the ground-dust gate
    /// measures the muzzle against (the shots below fire from y = 2.0).
    fn harness_ground(height: f32) -> App {
        let mut app = harness();
        let size = 33usize;
        app.insert_resource(crate::terrain_grid::HeightGrid::new(
            vec![height; size * size].into(),
            size as u32,
            crate::terrain_grid::FIXTURE_EXTENT,
        ));
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

    /// Run one frame of `secs` through both agers.
    fn advance(app: &mut App, secs: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(secs));
        app.update();
    }

    /// Every live billboard's RENDERED size, keyed by entity (spawn order) — the transform the
    /// frame actually draws, not the spec it was spawned from.
    fn scales(app: &mut App) -> std::collections::BTreeMap<Entity, Vec3> {
        let world = app.world_mut();
        let mut q = world.query_filtered::<(Entity, &Transform), With<Billboard>>();
        q.iter(world).map(|(entity, t)| (entity, t.scale)).collect()
    }

    /// An 88 shot over unknown ground spawns the airborne main-gun dressing — blast core + flash
    /// core + glow card + 2 planes + smoke (6 billboards) and 1 light (the ground-dust cloud needs
    /// a terrain surface;
    /// see `ground_dust_needs_a_low_barrel_over_known_ground`) — and an MG-calibre round gets the
    /// MG dressing instead: core +
    /// 1 flame plane (no smoke on the first round — the ration counts from 1) at a fraction of the
    /// 88's size, plus its own dim light (now on EVERY round). Each round is dressed by exactly ONE
    /// observer.
    #[test]
    fn main_gun_and_mg_split_the_dressing() {
        let mut app = harness();
        fire(&mut app, 0.088, 0);
        assert_eq!(
            billboards(&mut app),
            6,
            "88: blast core + flash core + glow card + 2 planes + smoke"
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

    /// Distance LOD contract: beyond `FAR_FULL_DRESSING` only the two cores + light spawn — the
    /// glow card, flame planes and smoke are all near-only. A camera parked well past the cutoff
    /// must leave the 88 with exactly TWO billboards (blast core + flash core) and its light.
    /// Guards the glow card against silently re-escaping the gate.
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
            2,
            "far 88: blast core + flash core only (no glow card, planes, or smoke)"
        );
        assert_eq!(
            lights(&mut app),
            1,
            "the muzzle light carries the read at range"
        );
    }

    /// The ground-dust puffs of the last shot in SPAWN order: `(entity, rendered diameter,
    /// rendered position)` each. The flipbook rate is the discriminator — no other muzzle layer
    /// plays at [`GROUND_DUST_FRAME_RATE`] — and the entity sort keeps a given puff at a stable
    /// index across ticks that despawn its neighbours.
    fn dust_puffs(app: &mut App) -> Vec<(Entity, f32, Vec3)> {
        let world = app.world_mut();
        let mut q = world.query::<(Entity, &Billboard, &Transform)>();
        let mut puffs: Vec<(Entity, f32, Vec3)> = q
            .iter(world)
            .filter(|(_, b, _)| b.frame_rate == GROUND_DUST_FRAME_RATE)
            .map(|(entity, _, t)| (entity, t.scale.x, t.translation))
            .collect();
        puffs.sort_unstable_by_key(|(entity, _, _)| *entity);
        puffs
    }

    /// The ground-dust gate: an 88 fired a couple of metres over known terrain lifts a cloud of
    /// [`GROUND_DUST_COUNT`] puffs sitting on that surface; the same shot with the ground far below
    /// lifts none, and an MG round never lifts dust at all.
    #[test]
    fn ground_dust_needs_a_low_barrel_over_known_ground() {
        // Muzzle at y = 2.0, ground at y = 0.0 — inside GROUND_DUST_MAX_HEIGHT.
        let mut low = harness_ground(0.0);
        fire(&mut low, 0.088, 0);
        let puffs = dust_puffs(&mut low);
        assert!(
            (GROUND_DUST_COUNT.0 as usize..=GROUND_DUST_COUNT.1 as usize).contains(&puffs.len()),
            "dust puff count {} outside {GROUND_DUST_COUNT:?}",
            puffs.len()
        );
        assert_eq!(
            billboards(&mut low),
            6 + puffs.len(),
            "the airborne dressing plus the cloud"
        );
        for (_, size, position) in &puffs {
            assert!(
                position.y >= 0.0 && position.y <= size * 0.5 + 1e-3,
                "a puff must hug the ground it was lifted from (y = {})",
                position.y
            );
        }

        // The same shot with the surface 6 m down: nothing to lift.
        let mut high = harness_ground(-6.0);
        fire(&mut high, 0.088, 0);
        assert!(dust_puffs(&mut high).is_empty(), "a high barrel lifts none");
        assert_eq!(billboards(&mut high), 6);

        // The MG's dressing has no ground layer at any height.
        let mut mg = harness_ground(0.0);
        fire_round(&mut mg, MG_CALIBER, 0, true);
        assert!(dust_puffs(&mut mg).is_empty(), "no dust off a rifle bore");
        assert_eq!(billboards(&mut mg), 2);
    }

    /// The cloud is scaled by the BORE, never by the viewer (ADR-0023): doubling the caliber
    /// doubles every metre the cloud renders — the puff's diameter at birth AND the height it has
    /// LIFTED after a second of drift (the vertical lift carries the same ratio as the radial
    /// push, so the cloud keeps its shape at any bore).
    #[test]
    fn ground_dust_scales_with_the_bore() {
        /// One shot at `caliber`: the first puff's rendered diameter at birth, and how far it has
        /// risen [`LIFT_SECS`] later.
        fn puff(caliber: f32) -> (f32, f32) {
            let mut app = harness_ground(0.0);
            fire(&mut app, caliber, 0);
            let (entity, size, born) = dust_puffs(&mut app)[0];
            // The arming frame, then the drift frame (see `age_billboards`).
            advance(&mut app, 0.0);
            advance(&mut app, LIFT_SECS);
            let lifted = app
                .world()
                .get::<Transform>(entity)
                .expect("alive")
                .translation;
            (size, lifted.y - born.y)
        }
        const LIFT_SECS: f32 = 1.0;

        let (small, small_lift) = puff(GROUND_DUST_CALIBER);
        assert!((small - GROUND_DUST_SIZE.0).abs() < 1e-4);
        assert!(
            (small_lift - GROUND_DUST_RISE * LIFT_SECS).abs() < 1e-4,
            "at the authored bore the lift IS GROUND_DUST_RISE (rose {small_lift} m in \
             {LIFT_SECS} s)"
        );

        let (big, big_lift) = puff(GROUND_DUST_CALIBER * 2.0);
        assert!(
            (big - small * 2.0).abs() < 1e-4,
            "twice the bore, twice the puff: {small} → {big}"
        );
        assert!(
            (big_lift - small_lift * 2.0).abs() < 1e-4,
            "twice the bore, twice the lift: {small_lift} → {big_lift}"
        );
    }

    /// The 88's curve contract, read off RENDERED state over real frames. Frame one draws every
    /// layer at its authored birth size — a newborn aged on its spawn frame would first draw a
    /// 26 ms core two thirds through its life, and below ~38 fps never draw it at all. One frame
    /// on, the HOT layers are collapsing while the MASS layers billow. Then the flash cluster dies
    /// on its three beats — core, both flame planes, glow card — with every mass layer still alive
    /// after the last of them.
    #[test]
    fn the_88_layers_are_staggered() {
        const FRAME: f32 = 1.0 / 60.0;
        let mut app = harness_ground(0.0);
        fire(&mut app, 0.088, 0);
        let born = scales(&mut app);
        let layers = born.len();

        advance(&mut app, FRAME);
        assert_eq!(
            scales(&mut app),
            born,
            "every layer's first rendered frame must be its authored birth size"
        );

        advance(&mut app, FRAME);
        let aged = scales(&mut app);
        assert_eq!(
            aged.len(),
            layers,
            "no layer dies inside its first 2 frames"
        );
        let hot = aged
            .iter()
            .filter(|(entity, scale)| scale.x < born[*entity].x)
            .count();
        assert_eq!(
            hot, 4,
            "flash core + 2 flame planes + glow card are born maximal and shrink"
        );
        assert_eq!(
            layers - hot,
            2 + dust_puffs(&mut app).len(),
            "the layers that grow — the blast fireball, gas smoke, ground dust"
        );

        // The three beats, each stepped just past its deadline (ages run from the arming frame):
        // core at 26 ms, both flame planes at 45 ms, glow card at 75 ms.
        advance(&mut app, 0.02);
        assert_eq!(billboards(&mut app), layers - 1, "the core dies first");
        advance(&mut app, 0.02);
        assert_eq!(billboards(&mut app), layers - 3, "both flame planes next");
        advance(&mut app, 0.03);
        assert_eq!(billboards(&mut app), layers - 4, "the glow card last");

        let survivors = scales(&mut app);
        assert!(!survivors.is_empty(), "the mass layers outlive the flash");
        for (entity, scale) in &survivors {
            assert!(
                scale.x > born[entity].x,
                "every layer alive past the flash cluster must be one that grows in"
            );
        }
    }

    /// The blast cores alive right now, in spawn order. [`BLAST_FRAMES`] is the discriminator — no
    /// other muzzle layer runs a 12-frame sheet.
    fn blast_cores(app: &mut App) -> Vec<Entity> {
        let world = app.world_mut();
        let mut q = world.query::<(Entity, &Billboard)>();
        let mut found: Vec<Entity> = q
            .iter(world)
            .filter(|(_, b)| b.frames == BLAST_FRAMES)
            .map(|(entity, _)| entity)
            .collect();
        found.sort_unstable();
        found
    }

    /// A billboard's per-instance material — the RENDERED atlas and flipbook frame live here, not
    /// on the [`Billboard`] component.
    fn material_of(app: &App, entity: Entity) -> &VfxBillboardMaterial {
        let handle = app
            .world()
            .get::<MeshMaterial3d<VfxBillboardMaterial>>(entity)
            .expect("material");
        app.world()
            .resource::<Assets<VfxBillboardMaterial>>()
            .get(&handle.0)
            .expect("per-instance material asset")
    }

    /// The blast core is the muzzle moment's dominant fireball: one per 88 shot, wider and
    /// longer-lived than the flash core it sits over, on the atlas's 4×3 grid.
    #[test]
    fn blast_core_dominates_the_flash_cluster() {
        let mut app = harness();
        fire(&mut app, 0.088, 0);
        let cores = blast_cores(&mut app);
        assert_eq!(cores.len(), 1, "one blast core per 88 shot");
        let core = app.world().get::<Billboard>(cores[0]).expect("blast core");
        assert!(
            core.start_size > FLASH_CORE_SIZE.1,
            "the blast must be wider than the widest flash core ({} m vs {} m)",
            core.start_size,
            FLASH_CORE_SIZE.1
        );
        assert!(
            core.lifetime > FLASH_GLOW_CARD_LIFETIME,
            "the blast must outlive the whole flash cluster ({} s vs {FLASH_GLOW_CARD_LIFETIME} s)",
            core.lifetime
        );
        let params = &material_of(&app, cores[0]).params;
        assert_eq!(
            (params.frame.y, params.frame.z),
            (BLAST_COLS, BLAST_ROWS),
            "the shader's cell arithmetic must read the atlas's real grid"
        );
        // An MG round never lifts a blast core — the fireball is the main gun's.
        let mut mg = harness();
        fire_round(&mut mg, MG_CALIBER, 0, true);
        assert!(
            blast_cores(&mut mg).is_empty(),
            "no fireball off a rifle bore"
        );
    }

    /// The blast plays ONCE: frame one renders the ignition frame unaged (the newborn-armed latch),
    /// the index only ever advances, and it is still inside the last cell when the quad dies — a
    /// wrap would re-ignite the fireball on its way out.
    #[test]
    fn blast_core_plays_its_sheet_once() {
        const STEP: f32 = 1.0 / 240.0;
        let mut app = harness();
        fire(&mut app, 0.088, 0);
        let core = blast_cores(&mut app)[0];
        assert_eq!(
            material_of(&app, core).params.frame.x,
            0.0,
            "the sheet opens on its most violent frame"
        );

        // The arming frame leaves it there; ageing starts after (see `age_billboards`).
        advance(&mut app, STEP);
        assert_eq!(material_of(&app, core).params.frame.x, 0.0);

        let mut seen = vec![0.0f32];
        while app.world().get::<Billboard>(core).is_some() {
            advance(&mut app, STEP);
            if app.world().get::<Billboard>(core).is_none() {
                break;
            }
            let frame = material_of(&app, core).params.frame.x;
            assert!(
                frame >= *seen.last().expect("seeded"),
                "the flipbook wrapped ({} after {})",
                frame,
                seen.last().expect("seeded")
            );
            seen.push(frame);
        }
        assert_eq!(
            *seen.last().expect("seeded"),
            (BLAST_FRAMES - 1) as f32,
            "the sheet must reach its last cell before the quad dies"
        );
    }

    /// Which of the two sheets plays is the blast's anti-repetition trick (its own sequence is
    /// fixed), so a burst of shots must draw both.
    #[test]
    fn blast_core_draws_both_sheets() {
        let mut app = harness();
        for _ in 0..12 {
            fire(&mut app, 0.088, 0);
        }
        let cores = blast_cores(&mut app);
        assert_eq!(cores.len(), 12, "one blast core per shot, none evicted");
        let sheets: std::collections::BTreeSet<_> = cores
            .iter()
            .map(|core| material_of(&app, *core).atlas.id())
            .collect();
        assert_eq!(sheets.len(), 2, "both blast sheets must reach the screen");
    }

    /// The fireball is scaled by the BORE, never by the viewer (ADR-0023): doubling the caliber
    /// doubles the quad and the lifetime, and the flipbook rate halves with it so the sheet still
    /// plays exactly once.
    #[test]
    fn blast_core_scales_with_the_bore() {
        fn core(caliber: f32) -> (f32, f32, f32) {
            let mut app = harness();
            fire(&mut app, caliber, 0);
            let entity = blast_cores(&mut app)[0];
            let core = app.world().get::<Billboard>(entity).expect("blast core");
            (core.start_size, core.lifetime, core.frame_rate)
        }

        let (small, small_life, small_rate) = core(BLAST_CALIBER);
        let (big, big_life, big_rate) = core(BLAST_CALIBER * 2.0);
        assert!(
            (big - small * 2.0).abs() < 1e-3,
            "twice the bore, twice the fireball: {small} → {big}"
        );
        assert!(
            (big_life - small_life * 2.0).abs() < 1e-4,
            "twice the bore, twice the burn: {small_life} → {big_life}"
        );
        assert!(
            (big_rate * 2.0 - small_rate).abs() < 1e-3,
            "the sheet is stretched over the longer life, not replayed: {small_rate} → {big_rate}"
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
        // Under the shipped lever the MG light is present but does NOT cast — the whole point of
        // the 2026-07-31 measurement (see [`MuzzleShadows`]).
        assert!(
            !point.shadow_maps_enabled,
            "the shipped lever spares the MG light its shadow cubemap"
        );
    }

    /// The shadow lever: `MainGunOnly` (shipped) casts on the 88 alone, `On` casts on both guns,
    /// `Off` casts on neither. The MG arm is the cost-bearing one — an MG round's light casting is
    /// six cubemap faces of the shooter's own Tiger at 750 rpm.
    #[test]
    fn shadow_lever_gates_casting() {
        /// A `(main gun casts, MG casts)` read of one fire of each gun under `mode`.
        fn casting(mode: MuzzleShadows) -> (bool, bool) {
            let mut main = harness_shadows(mode);
            fire(&mut main, 0.088, 0);
            let world = main.world_mut();
            let mut q = world.query::<&PointLight>();
            let main_casts = q.single(world).expect("one 88 light").shadow_maps_enabled;

            let mut mg = harness_shadows(mode);
            fire_round(&mut mg, MG_CALIBER, 0, true);
            let world = mg.world_mut();
            let mut q = world.query::<&PointLight>();
            let mg_casts = q.single(world).expect("one MG light").shadow_maps_enabled;
            (main_casts, mg_casts)
        }

        assert_eq!(
            casting(MuzzleShadows::MainGunOnly),
            (true, false),
            "the shipped lever: the 88's flash casts, the MG's glimmer does not"
        );
        assert_eq!(
            casting(MuzzleShadows::default()),
            (true, false),
            "MainGunOnly IS the default — the A/B arms are opt-in only"
        );
        assert_eq!(casting(MuzzleShadows::On), (true, true), "On: both cast");
        assert_eq!(
            casting(MuzzleShadows::Off),
            (false, false),
            "Off: neither casts"
        );
    }

    /// EVERY policy is nameable, and the name the client LOGS is the name the knob ACCEPTS. Both
    /// halves are load-bearing for `scripts/perf/run-fire-capture.sh`, whose arms name a policy in
    /// the env and then verify the client resolved that same token — an A/B whose "shipped" arm
    /// could not name the shipped policy is how that script came to measure the legacy one twice.
    #[test]
    fn every_shadow_policy_has_a_token_that_round_trips() {
        for mode in [
            MuzzleShadows::MainGunOnly,
            MuzzleShadows::On,
            MuzzleShadows::Off,
        ] {
            assert_eq!(
                MuzzleShadows::parse(Some(mode.token())),
                mode,
                "the logged token must parse back to the policy that logged it",
            );
        }
        // Unset (the shipped default) and an unrecognized value both land on the default — the
        // latter is caught by the runner's token check, not by a parse failure.
        assert_eq!(MuzzleShadows::parse(None), MuzzleShadows::default());
        assert_eq!(
            MuzzleShadows::parse(Some("main_only")),
            MuzzleShadows::MainGunOnly,
            "an unrecognized value falls back to the default (and logs `main-only`)",
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
        assert_eq!(billboards(&mut app), 6);
    }

    /// The muzzle light decays monotonically from its first-frame peak and despawns at end of
    /// life — the "first frame hottest" contract, which holds through a whole rendered frame: a
    /// light aged on its spawn frame would first draw a third of the way down its 55 ms envelope.
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

        // Frame one at 60 fps — a third of the light's whole life, and it is still at peak.
        advance(&mut app, 1.0 / 60.0);
        let world = app.world_mut();
        let mut q = world.query::<&PointLight>();
        assert_eq!(
            q.single(world).expect("alive on frame one").intensity,
            LIGHT_PEAK_LUMENS,
            "the first RENDERED frame is the peak, not one frame down the decay"
        );

        advance(&mut app, LIGHT_LIFETIME * 0.5);
        let world = app.world_mut();
        let mut q = world.query::<(&MuzzleLight, &PointLight)>();
        let (_, point) = q.single(world).expect("still alive mid-decay");
        assert!(
            point.intensity < LIGHT_PEAK_LUMENS * 0.5,
            "cubic decay front-loads the drop"
        );

        advance(&mut app, LIGHT_LIFETIME);
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
