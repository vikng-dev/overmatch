//! The game's damage-model view, game state only.
//!
//! View-only muzzle dressing for [`FireShell`] events: one consumer for every gun, reading the
//! shot's resolved emission ([`crate::guns::MuzzleEmission`], carried on the shot): the muzzle
//! law's sizes for its muzzle energy and its gun's muzzle shape (`guns::muzzle`; the law is the
//! design record's "The muzzle law", `.agents/docs/design/view-sweep-review.md`). No calibre,
//! weapon kind or viewer distance selects a layer's size, life or light; the constants below are
//! the shipped sprite sheets' playback, erosion rules and renderer caps; sprite taste is
//! `MuzzleVfxAssets::renderer`, loaded from the same taste file as the emission law.
//!
//! Layers: the flash core, the fireball, the fill glow, the flame planes (a forward jet and any
//! side jets), the gas smoke, the ground blast's dust cloud and the transient light. The core, the
//! fireball and the light draw at any range; beyond [`FAR_FULL_DRESSING`] the other layers are
//! skipped, a cost bound on overdraw that never resizes what is drawn.
//!
//! Invariant (ADR-0014): spawned entities are render-only. Light and billboard rings bound replay
//! duplicates; stale remote shots do not replay an already-expired flash.

use std::collections::BTreeMap;

use bevy::camera::Exposure;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

// `STALE_FIRE_TICKS` is shared with the sim-side catch-up impact gate (`ballistics::on_fire_shell`)
// so the muzzle flash and the impact phantom fall stale together — one constant, no drift.
use crate::ballistics::{FireShell, STALE_FIRE_TICKS};
use crate::guns::{CoreSprite, GroundBlast};
use crate::spec::FireCycle;
use crate::tank::{TankRoot, Weapon, WeaponIndex};
use crate::view::PlayerView;

use super::ViewRng;
use super::billboard::{
    BillboardLook, BillboardMaterials, BillboardRing, BillboardSpec, Flipbook, SOFT_PARTICLE_DEPTH,
    VfxParams, gradient_lut, smoothstep, spawn_billboard, unit_quad,
};

// --- The fireball sheet: a 12-frame explosion flipbook played once over the emission's lifetime.

/// Atlas grid and frame count — [`MuzzleVfxAssets::blast_atlas`]'s two sheets share it
/// (`scripts/vfx/blast_atlas.py`).
const BLAST_COLS: f32 = 4.0;
const BLAST_ROWS: f32 = 3.0;
const BLAST_FRAMES: u32 = 12;
/// How many baked sheets are in the rotation — [`MuzzleVfxAssets::blast_atlas`]'s length and
/// [`blast_pick`]'s range.
const BLAST_SHEETS: usize = 2;
/// Erosion threshold at death. Low, unlike every other muzzle layer: the sheet ALREADY animates its
/// own dissolution, and the die-off is the blast LUT's cooling term (which reaches black at the last
/// LUT row, so the layer is gone before it despawns). Erosion here only thins the fringe — ramped to
/// 1.0 it instead eats the fireball into confetti through the middle of the sequence.
const BLAST_EROSION: f32 = 0.3;

/// Smoke sprite playback: frames/s over the 4-frame atlas, roll-rate bound (rad/s).
const SMOKE_FRAME_RATE: f32 = 5.0;
const SMOKE_SPIN_MAX: f32 = 0.6;

/// Shared muzzle-light population cap; oldest lights are evicted first.
const LIGHT_CAP: usize = 12;

/// Dust sprite playback: frames/s over the 4-frame dust atlas, and its roll-rate bound (rad/s).
const GROUND_DUST_FRAME_RATE: f32 = 3.0;
const GROUND_DUST_SPIN_MAX: f32 = 0.35;

/// The erosion threshold a dust puff reaches at the end of its life: the dust sheet's own signal
/// ceiling, so a puff dissolves over its whole resolved lifetime. MEASURED on `impact_dust.png`'s
/// signal (red × alpha) histogram under the shader's erosion at this layer's sharpness: a ramp to
/// 1.0 halves a puff's coverage at 17 % of its life and quarters it at 28 %; a ramp to 0.5
/// quarters it at 55 % and leaves 5 % at 90 %.
const GROUND_DUST_EROSION_END: f32 = 0.5;

/// Beyond this camera distance (m) only the core, the fireball and the light spawn: a cost bound on
/// near-field overdraw. It skips layers and never changes the size of a drawn one.
const FAR_FULL_DRESSING: f32 = 400.0;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<MuzzleLightRing>()
        .init_resource::<SmokeCadence>()
        .init_resource::<DustPalettes>()
        .add_systems(Startup, setup_muzzle_assets)
        .add_observer(dress_fire)
        .add_systems(Update, decay_muzzle_lights);
}

/// Preloaded muzzle-dressing assets: the shared quad, the two sprite atlases, and the per-effect
/// gradient LUTs (one grayscale sprite set, recolored per effect — the LUT trick).
#[derive(Resource)]
pub(super) struct MuzzleVfxAssets {
    pub(super) renderer: crate::guns::MuzzleRenderer,
    pub(super) quad: Handle<Mesh>,
    /// The starburst core: a 2×2 scorch-starburst atlas (a spiky radial star per shot).
    pub(super) core_atlas: Handle<Image>,
    /// The fireball: two 4×3 explosion flipbooks, one picked per shot. Two sheets rather
    /// than one is the anti-repetition trick at sprite scale — the sequence inside a sheet is
    /// fixed (it starts on its most violent frame and plays once), so the variation has to come
    /// from WHICH fireball plays. KTX2, unlike every other sprite here: this one is the only
    /// billboard drawn at unbounded range, so it is the only one that needs a mip chain (bevy's
    /// PNG loader produces a single level) — see `scripts/vfx/blast_atlas.py`.
    blast_atlas: [Handle<Image>; BLAST_SHEETS],
    /// The round-glow core and the fill glow: a single small round glow (`light_01`).
    mg_core: Handle<Image>,
    flame_atlas: Handle<Image>,
    smoke_atlas: Handle<Image>,
    /// The billow atlas the impact read also loads (the asset server dedupes the load and shares
    /// the GPU texture) — the mass sprite the ground-dust cloud is drawn from, through the lifting
    /// ground's own palette ([`DustPalettes`]).
    dust_atlas: Handle<Image>,
    flash_lut: Handle<Image>,
    /// The blast core's own palette — the flash LUT drives nearly the whole signal band to
    /// white-hot, which is the read a 2-frame starburst wants and the wrong one for a 12-frame
    /// sheet whose MID-tones are the fire.
    blast_lut: Handle<Image>,
    smoke_lut: Handle<Image>,
}

impl MuzzleVfxAssets {
    /// The flash material template (additive — hot cores never darken; survey trick 9).
    pub(super) fn flash_material(&self, atlas: Handle<Image>, sharpness: f32) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, sharpness, 0.0, 1.0),
                // glow.y = 1.0: the additive blend contract (see `vfx_billboard.wgsl`) — premultiply
                // by coverage so transparent texels add nothing (kills the old orange square).
                glow: Vec4::new(self.renderer.flash_boost, 1.0, 0.0, 0.0),
            },
            atlas,
            lut: self.flash_lut.clone(),
            alpha_mode: AlphaMode::Add,
        }
    }

    /// The blast-core material template: one of the two flipbook sheets through the blast LUT.
    /// Rides the flash's additive pipeline (no new permutation), with its own 4×3 grid lanes,
    /// softer erosion and a lower boost.
    fn blast_material(&self, atlas: Handle<Image>) -> BillboardLook {
        let mut material = self.flash_material(atlas, self.renderer.fireball_sharpness);
        material.params.frame = Vec4::new(0.0, BLAST_COLS, BLAST_ROWS, 0.0);
        material.params.fade.w = self.renderer.fireball_opacity;
        material.params.glow.x = self.renderer.fireball_boost;
        material.lut = self.blast_lut.clone();
        material
    }

    /// The smoke material template (alpha-blend — smoke is mass, it darkens and occludes).
    pub(super) fn smoke_material(&self) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                // Moderate sharpness: soft dissolve edges on the puff. w, the overall opacity, is
                // the emission's: the consumer writes it per puff.
                fade: Vec4::new(0.0, self.renderer.smoke_sharpness, 0.0, 1.0),
                glow: Vec4::new(self.renderer.smoke_boost, 0.0, SOFT_PARTICLE_DEPTH, 0.0),
            },
            atlas: self.smoke_atlas.clone(),
            lut: self.smoke_lut.clone(),
            alpha_mode: AlphaMode::Blend,
        }
    }

    /// The ground-dust material template at overall `opacity`: the billow atlas through the lifting
    /// ground's `palette` ([`DustPalettes`]), alpha-blend occluding mass. Lifted soil is lit mass,
    /// never an emitter: the palette's gain lane is one everywhere, so the shader's
    /// `1 + gain · glow.x` is `light`, the key light's radiance per unit albedo. Rides the smoke's
    /// already-warmed Blend pipeline.
    fn dust_material(&self, opacity: f32, palette: Handle<Image>, light: f32) -> BillboardLook {
        BillboardLook {
            params: VfxParams {
                frame: Vec4::new(0.0, 2.0, 2.0, 0.0),
                fade: Vec4::new(0.0, self.renderer.dust_sharpness, 0.0, opacity),
                glow: Vec4::new(light - 1.0, 0.0, SOFT_PARTICLE_DEPTH, 0.0),
            },
            atlas: self.dust_atlas.clone(),
            lut: palette,
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
    commands.insert_resource(MuzzleVfxAssets {
        renderer: crate::guns::MuzzleLook::shipped().renderer,
        quad: unit_quad(&mut meshes),
        core_atlas: asset_server.load(super::Texture::FlashCore.path()),
        blast_atlas: [
            asset_server.load(super::Texture::BlastCoreA.path()),
            asset_server.load(super::Texture::BlastCoreB.path()),
        ],
        mg_core: asset_server.load(super::Texture::MgCore.path()),
        flame_atlas: asset_server.load(super::Texture::FlashFlames.path()),
        smoke_atlas: asset_server.load(super::Texture::Smoke.path()),
        dust_atlas: asset_server.load(super::Texture::ImpactDust.path()),
        flash_lut,
        blast_lut,
        smoke_lut,
    });
}

/// One palette per lifted-soil colour ([`crate::ground_types::DustResponse::colour_srgb`]), made
/// the first time that colour's ground lifts dust and kept for the session: the ground table is
/// the colours' one home, and a tuned table makes its new colour's palette on its next shot.
#[derive(Resource, Default)]
struct DustPalettes(BTreeMap<[u32; 3], Handle<Image>>);

impl DustPalettes {
    /// The palette of soil of albedo `colour_srgb`: the albedo where the sprite signal is strong
    /// (the sunlit face), tinted by the key light's linear colour, as Bevy lights the terrain.
    /// ASSUMED cloud shading: a shadow floor of 0.16 of lit soil, ageing to 0.76 (Y).
    /// These are presentation contrast choices, not measured soil properties. The gain lane
    /// is one: the light's brightness and exposure ride the material.
    fn palette(&mut self, images: &mut Assets<Image>, colour_srgb: [f32; 3]) -> Handle<Image> {
        self.0
            .entry(colour_srgb.map(f32::to_bits))
            .or_insert_with(|| {
                let albedo = LinearRgba::from(Color::srgb_from_array(colour_srgb));
                // The world key's colour is constant for the session; only its elevation varies.
                let sun = crate::world::sun_light().color.to_linear();
                gradient_lut(images, |x, y| {
                    let shade = (0.16 + 0.84 * x) * (0.76 + 0.24 * (1.0 - y));
                    (
                        LinearRgba::rgb(
                            albedo.red * sun.red * shade,
                            albedo.green * sun.green * shade,
                            albedo.blue * sun.blue * shade,
                        ),
                        1.0,
                    )
                })
            })
            .clone()
    }
}

/// A live muzzle light's age plus the scale it was born with; [`decay_muzzle_lights`] drives the
/// intensity fall and the despawn from these. Per-light `peak`/`lifetime` is what lets every gun
/// share one decay system at its own scale.
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

/// ASSUMED shadow-read window (s): a persistent light or an isolated flash casts; repeated
/// flashes shorter than this do not. MEASURED evidence for the short/repeating regime:
/// `scripts/perf/run-fire-capture.sh`, M4, two tanks, M350, vsync off: firing with shadows
/// moved p50 from 13.5 to 15.5 ms, without from 13.5 to 14.6 ms. DERIVED at the fixture's
/// combined rate of 25/s and 0.05 s life, each cubemap lasts about three 60 Hz frames.
/// This window is a presentation policy, not a measured perceptual threshold.
const SHADOW_READ_WINDOW_S: f32 = 0.1;

/// The game's damage-model flash's shortest refire interval, from its loaded weapon data.
/// An unattributed dev/test shot, or a delivered shot whose weapon has left the scene, is an
/// isolated flash with no current repeating source.
fn refire_interval(
    fire: &FireShell,
    weapons: &Query<(&Weapon, &TankRoot, &WeaponIndex)>,
) -> Option<f32> {
    let source = fire.shooter?;
    let Some((weapon, _, _)) = weapons
        .iter()
        .find(|(_, root, slot)| root.0 == source.tank && slot.0 == source.weapon)
    else {
        // A fire cause may outlive its weapon: presentation is delivered after the source left
        // or its queued teardown ran. This orphaned flash has no live cadence to repeat at.
        return None;
    };
    Some(
        match weapon.fire_cycle {
            FireCycle::Single { reload } => reload,
            FireCycle::Automatic { cyclic, .. } => cyclic,
        }
        .secs(),
    )
}

/// Spawn a transient light into the shared ring. Its life and minimum refire interval decide
/// whether a shadow can read; weapon kind does not enter the decision.
fn spawn_muzzle_light(
    commands: &mut Commands,
    ring: &mut MuzzleLightRing,
    position: Vec3,
    light: &crate::guns::FlashLight,
    refire_interval: Option<f32>,
) {
    let lifetime = light.lifetime_s;
    let peak = light.peak_lm;
    let [red, green, blue] = light.colour_srgb;
    let light = commands
        .spawn((
            MuzzleLight {
                age: 0.0,
                armed: false,
                peak,
                lifetime,
            },
            PointLight {
                color: Color::srgb(red, green, blue),
                intensity: peak,
                range: light.range_m,
                radius: light.radius_m,
                shadow_maps_enabled: lifetime >= SHADOW_READ_WINDOW_S
                    || refire_interval.is_none_or(|interval| interval >= SHADOW_READ_WINDOW_S),
                ..default()
            },
            // "The hull occludes it like any object" is only true while the light can SEE the hull.
            // With the vendored shadow-view patch a casting light's shadow view inherits this
            // mask — so without a profile it would neither light nor be occluded
            // by the player's own tank (which lives on its own channel), and the track ribbon would
            // stop casting from it too.
            crate::render_policy::LightProfile::BattlefieldMuzzleFlash,
            Transform::from_translation(position),
        ))
        .id();
    crate::push_capped_entity(commands, &mut ring.0, light, LIGHT_CAP);
}

/// Every presented shot's counter for a mount's smoke ration
/// ([`crate::guns::MuzzleEmission::smoke`]'s `every_rounds`); it ticks once per non-stale shot.
#[derive(Resource, Default)]
struct SmokeCadence(std::collections::HashMap<Option<(Entity, usize)>, u32>);

impl SmokeCadence {
    fn puff(&mut self, source: Option<crate::ballistics::ShotSource>, every: u32) -> bool {
        // Unattributed developer shots form one stream; game shots name their tank and mount.
        let key = source.map(|source| (source.tank, source.weapon));
        let count = self.0.entry(key).or_default();
        if *count == every - 1 {
            *count = 0;
            true
        } else {
            *count += 1;
            false
        }
    }
}

/// What the one muzzle consumer writes into and reads beside the shot.
#[derive(SystemParam)]
struct MuzzleKit<'w, 's> {
    assets: Res<'w, MuzzleVfxAssets>,
    materials: BillboardMaterials<'w>,
    ring: ResMut<'w, BillboardRing>,
    light_ring: ResMut<'w, MuzzleLightRing>,
    cadence: ResMut<'w, SmokeCadence>,
    rng: ResMut<'w, ViewRng>,
    camera: Query<'w, 's, (&'static GlobalTransform, Option<&'static Exposure>), With<PlayerView>>,
    weapons: Query<'w, 's, (&'static Weapon, &'static TankRoot, &'static WeaponIndex)>,
    /// The decoded terrain surface, when the world has one: the ground blast's only height query.
    grid: Option<Res<'w, crate::terrain_grid::HeightGrid>>,
    /// The ground-type table and its painted mask, which `track_prints::install` inserts together
    /// in the drawn heightmap world. A composition with no drawn ground (no window) has neither,
    /// states no erodibility and lifts no dust.
    painted: Option<(
        Res<'w, crate::ground_types::GroundTypes>,
        Res<'w, crate::track_prints::PrintDepths>,
    )>,
    palettes: ResMut<'w, DustPalettes>,
    images: ResMut<'w, Assets<Image>>,
    commands: Commands<'w, 's>,
}

impl MuzzleKit<'_, '_> {
    fn billboard(&mut self, spec: BillboardSpec) {
        spawn_billboard(
            &mut self.commands,
            &mut self.materials,
            &mut self.ring,
            self.assets.quad.clone(),
            spec,
        );
    }
}

/// Dress one shot from its resolved emission, hung off the `FireShell` geometry (origin and
/// bore direction).
fn dress_fire(fire: On<FireShell>, mut kit: MuzzleKit) {
    // Stale remote shot: the flash moment is long past — skip, don't play late.
    if fire.catch_up_ticks > STALE_FIRE_TICKS {
        return;
    }
    let emission = fire.muzzle;
    // The game's damage-model view counts this mount's non-stale shots, near or far. Other
    // mounts and smokeless shots cannot change its ration.
    let smoke_due = emission
        .smoke
        .is_some_and(|smoke| kit.cadence.puff(fire.shooter, smoke.every_rounds));
    let origin = fire.origin;
    let dir = Vec3::from(fire.direction);
    // The headless effect harness has no viewer: it exercises the complete recipe, under the
    // exposure Bevy renders a camera without an `Exposure` at.
    let (near, exposure) = if kit.camera.is_empty() {
        (true, Exposure::default())
    } else {
        let (eye, exposure) = kit
            .camera
            .single()
            .expect("exactly one camera declares PlayerView");
        (
            eye.translation().distance_squared(origin) < FAR_FULL_DRESSING * FAR_FULL_DRESSING,
            // Bevy's own rule (`bevy_render::camera::extract_cameras`): a camera with no
            // `Exposure` renders at the default one.
            exposure.copied().unwrap_or_default(),
        )
    };

    // --- Flash core: one camera-facing additive billboard, born at its largest. The starburst
    // atlas gets a random one of its four frames per shot (nothing lives long enough to animate);
    // random roll and size, so no two shots match.
    let core = emission.core;
    let core_size = kit.rng.range(core.diameter_m.0, core.diameter_m.1);
    let (core_material, core_frames, core_start) = match core.sprite {
        CoreSprite::Starburst => (
            kit.assets.flash_material(
                kit.assets.core_atlas.clone(),
                kit.assets.renderer.core_sharpness,
            ),
            4,
            kit.rng.range(0.0, 4.0).floor(),
        ),
        CoreSprite::Glow => {
            let mut material = kit.assets.flash_material(
                kit.assets.mg_core.clone(),
                kit.assets.renderer.core_sharpness,
            );
            // The glow is the whole image, not an atlas cell.
            material.params.frame = Vec4::new(0.0, 1.0, 1.0, 0.0);
            (material, 1, 0.0)
        }
    };
    let core_roll = kit.rng.range(0.0, std::f32::consts::TAU);
    kit.billboard(BillboardSpec {
        material: core_material,
        lifetime: core.lifetime_s,
        origin: origin + dir * core.standoff_m,
        drift: Vec3::ZERO,
        frames: core_frames,
        flipbook: Flipbook::Loop {
            start: core_start,
            rate: 0.0,
        },
        start_size: core_size,
        end_size: core_size * core.end_scale,
        aspect: Vec3::ONE,
        roll: core_roll,
        spin: 0.0,
        erosion_end: 0.0,
        rotation: None,
    });

    // --- Fireball: the flipbook, played ONCE from its most violent frame. Drawn at any range: it
    // is the shot's primary read, one quad per shot.
    if let Some(fireball) = emission.fireball {
        let size = kit.rng.range(fireball.diameter_m.0, fireball.diameter_m.1);
        let sheet = blast_pick(&mut kit.rng);
        let roll = kit.rng.range(0.0, std::f32::consts::TAU);
        let material = kit
            .assets
            .blast_material(kit.assets.blast_atlas[sheet].clone());
        kit.billboard(BillboardSpec {
            material,
            lifetime: fireball.lifetime_s,
            origin: origin + dir * fireball.standoff_m,
            drift: Vec3::ZERO,
            frames: BLAST_FRAMES,
            // A real sequence from its ignition frame: `Once` spreads the sheet over the
            // emission's lifetime, so the playback needs no rate of its own.
            flipbook: Flipbook::Once,
            start_size: size,
            end_size: size * fireball.growth,
            aspect: Vec3::ONE,
            roll,
            spin: 0.0,
            erosion_end: BLAST_EROSION,
            rotation: None,
        });
    }

    // --- Fill glow: one soft additive round-glow billboard behind the core, sized off it, eroding
    // out over its own life.
    if near && let Some(glow_card) = emission.glow {
        let mut glow = kit.assets.flash_material(
            kit.assets.mg_core.clone(),
            kit.assets.renderer.glow_sharpness,
        );
        glow.params.frame = Vec4::new(0.0, 1.0, 1.0, 0.0);
        glow.params.glow.x = kit.assets.renderer.glow_boost;
        glow.params.fade.w = kit.assets.renderer.glow_opacity;
        let size = core_size * glow_card.scale;
        let roll = kit.rng.range(0.0, std::f32::consts::TAU);
        kit.billboard(BillboardSpec {
            material: glow,
            lifetime: glow_card.lifetime_s,
            origin: origin + dir * core.standoff_m,
            drift: Vec3::ZERO,
            frames: 1,
            flipbook: Flipbook::Loop {
                start: 0.0,
                rate: 0.0,
            },
            start_size: size,
            end_size: size * glow_card.end_scale,
            aspect: Vec3::ONE,
            roll,
            spin: 0.0,
            erosion_end: 1.0,
            rotation: None,
        });
    }

    // --- Flame planes: each jet's quads evenly crossed around its axis, on consecutive frames of
    // the 4-flame atlas from a random start. The forward jet runs down the bore; side jets leave
    // in the plane of the bore and the side axis, at the shape's angle from the bore.
    let flames = emission.flames;
    if near {
        let mut jets = vec![(dir, flames.forward)];
        if let Some(side) = flames.side_jets {
            // The side axis is level across the bore; a vertical bore has no level side, and
            // any axis across it serves.
            let across = dir
                .cross(Vec3::Y)
                .try_normalize()
                .unwrap_or_else(|| dir.any_orthonormal_vector());
            let (sin, cos) = side.angle_rad.sin_cos();
            for flank in [across, -across] {
                jets.push((dir * cos + flank * sin, side.jet));
            }
        }
        let plane_frame = kit.rng.range(0.0, 4.0).floor();
        for (axis, jet) in jets {
            spawn_flame_jet(&mut kit, origin, axis, jet, &flames, plane_frame);
        }
    }

    // --- Gas smoke: one alpha-blended eroding puff on the gun's ration, rising and pushed along
    // the bore.
    if near
        && let Some(smoke) = emission.smoke
        && smoke_due
    {
        let mut material = kit.assets.smoke_material();
        material.params.fade.w = smoke.opacity;
        let start = kit.rng.range(0.0, 4.0);
        let roll = kit.rng.range(0.0, std::f32::consts::TAU);
        let spin = kit.rng.range(-SMOKE_SPIN_MAX, SMOKE_SPIN_MAX);
        kit.billboard(BillboardSpec {
            material,
            lifetime: smoke.lifetime_s,
            origin: origin + dir * smoke.standoff_m,
            drift: Vec3::Y * smoke.rise_mps + dir * smoke.push_mps,
            frames: 4,
            flipbook: Flipbook::Loop {
                start,
                rate: SMOKE_FRAME_RATE,
            },
            start_size: smoke.diameter_m.0,
            end_size: smoke.diameter_m.1,
            aspect: Vec3::ONE,
            roll,
            spin,
            erosion_end: 1.0,
            rotation: None,
        });
    }

    // --- Ground blast: the gas lifting the ground under a low barrel, as much as that ground
    // gives up.
    if near
        && let Some(blast) = emission.ground_blast
        && let Some(ground_y) = dust_ground_y(kit.grid.as_deref(), origin, blast.reach_m)
        && let Some((table, painted)) = kit.painted.as_ref()
    {
        let weights = painted.ground_weights(Vec2::new(origin.x, origin.z));
        let sources = crate::terrain_blend::dust_sources(table, weights);
        // The key light's radiance per unit albedo on level ground: what lit soil of albedo one
        // draws at under this camera.
        let light = crate::world::level_ground_irradiance_lux() / std::f32::consts::PI
            * exposure.exposure();
        spawn_ground_dust(origin, dir, ground_y, sources, light, &blast, &mut kit);
    }

    // --- Muzzle light: transient, first frame hottest. Its lifetime and refire cadence decide
    // casting.
    let light = emission.light;
    let refire = refire_interval(&fire, &kit.weapons);
    spawn_muzzle_light(
        &mut kit.commands,
        &mut kit.light_ring,
        origin + dir * light.standoff_m,
        &light,
        refire,
    );
}

/// One flame jet along unit `axis` from the muzzle at `origin`: `jet.planes` bore-style quads
/// crossed around the axis, each its own rolled length, the quad's centre ~45% along the flame so
/// its base hugs the muzzle.
fn spawn_flame_jet(
    kit: &mut MuzzleKit,
    origin: Vec3,
    axis: Vec3,
    jet: crate::guns::FlameJet,
    flames: &crate::guns::Flames,
    plane_frame: f32,
) {
    if jet.planes == 0 {
        return;
    }
    let base_roll = kit.rng.range(0.0, std::f32::consts::TAU);
    let step = std::f32::consts::PI / f32::from(jet.planes);
    for i in 0..jet.planes {
        let length = kit.rng.range(jet.length_m.0, jet.length_m.1);
        // Sprite +Y (flame up) onto the axis, then rolled around it.
        let rotation = Quat::from_axis_angle(axis, base_roll + f32::from(i) * step)
            * Quat::from_rotation_arc(Vec3::Y, axis);
        let material = kit.assets.flash_material(
            kit.assets.flame_atlas.clone(),
            kit.assets.renderer.flame_sharpness,
        );
        kit.billboard(BillboardSpec {
            material,
            lifetime: flames.lifetime_s,
            origin: origin + axis * (length * 0.45),
            drift: Vec3::ZERO,
            frames: 4,
            flipbook: Flipbook::Loop {
                start: (plane_frame + f32::from(i)) % 4.0,
                rate: 0.0,
            },
            start_size: length,
            end_size: length * flames.end_scale,
            aspect: Vec3::new(flames.width_ratio, 1.0, 1.0),
            roll: 0.0,
            spin: 0.0,
            erosion_end: 0.0,
            rotation: Some(rotation),
        });
    }
}

/// Which of the two blast sheets this shot plays. A sheet's own sequence is fixed — it opens on
/// its most violent frame and runs once — so consecutive shots are kept apart by the pick, not by
/// the start frame the other layers randomize.
fn blast_pick(rng: &mut ViewRng) -> usize {
    (rng.next_f32() * BLAST_SHEETS as f32) as usize
}

/// The ground height (m) under `muzzle` when the gas reaches it, else `None`: no decoded terrain
/// (`grid`) and no ground is known, and outside the grid's span the world ends at the collider edge
/// — [`crate::terrain_grid::HeightGrid::height_at`] would hand back a clamped phantom there. A
/// muzzle higher than the shot's `reach_m` over that surface lifts nothing.
fn dust_ground_y(
    grid: Option<&crate::terrain_grid::HeightGrid>,
    muzzle: Vec3,
    reach_m: f32,
) -> Option<f32> {
    let grid = grid?;
    if !grid.contains_xz(muzzle.x, muzzle.z) {
        return None;
    }
    let ground = grid.height_at(muzzle.x, muzzle.z);
    (muzzle.y - ground <= reach_m).then_some(ground)
}

/// The ground blast's dust cloud: wide alpha-blend puffs born on the ground under the muzzle and
/// pushed outward, each along an azimuth pulled toward the bore's by the emission's `bore_bias`.
/// The painted grounds under the muzzle are the cloud's `sources` (`terrain_blend::dust_sources`):
/// their shares sum to the share of the shot's rolled puff count the ground lifts, and each puff
/// is one source's soil, drawn by share, in that ground's albedo lit by `light`.
fn spawn_ground_dust(
    muzzle: Vec3,
    dir: Vec3,
    ground_y: f32,
    sources: [(f32, crate::ground_types::DustResponse); 4],
    light: f32,
    blast: &GroundBlast,
    kit: &mut MuzzleKit,
) {
    let base = Vec3::new(muzzle.x, ground_y, muzzle.z);
    // The bore's ground-plane azimuth; a vertical barrel has none, and the cloud stays a plain ring.
    let bore_xz = Vec3::new(dir.x, 0.0, dir.z).try_normalize();
    let (lo, hi) = blast.puffs;
    let rolled = (lo + (kit.rng.next_f32() * (hi - lo + 1) as f32) as u32).min(hi);
    let lifted: f32 = sources.iter().map(|(share, _)| share).sum();
    let count = (rolled as f32 * lifted).round() as u32;
    for _ in 0..count {
        let soil = soil_of(&sources, kit.rng.next_f32() * lifted);
        let theta = kit.rng.range(0.0, std::f32::consts::TAU);
        let radial = Vec3::new(theta.cos(), 0.0, theta.sin());
        let away = match bore_xz {
            Some(bore_dir) => (radial * (1.0 - blast.bore_bias) + bore_dir * blast.bore_bias)
                .try_normalize()
                .expect(
                    "the gun file's bore bias exceeds one half, so its unit terms cannot cancel",
                ),
            None => radial,
        };
        let start_size = blast.birth_diameter_m;
        let end_size = kit
            .rng
            .range(blast.end_diameter_m.0, blast.end_diameter_m.1);
        let offset = kit.rng.range(blast.spread_m.0, blast.spread_m.1);
        let push = kit.rng.range(blast.push_mps.0, blast.push_mps.1);
        let lifetime = kit.rng.range(blast.lifetime_s.0, blast.lifetime_s.1);
        let start = kit.rng.range(0.0, 4.0);
        let roll = kit.rng.range(0.0, std::f32::consts::TAU);
        let spin = kit.rng.range(-GROUND_DUST_SPIN_MAX, GROUND_DUST_SPIN_MAX);
        let palette = kit.palettes.palette(&mut kit.images, soil.colour_srgb);
        let material = kit.assets.dust_material(blast.opacity, palette, light);
        kit.billboard(BillboardSpec {
            material,
            lifetime,
            // Half a birth diameter up: the puff's lower edge sits ON the surface.
            origin: base + away * offset + Vec3::Y * (start_size * 0.5),
            drift: away * push + Vec3::Y * blast.rise_mps,
            frames: 4,
            flipbook: Flipbook::Loop {
                start,
                rate: GROUND_DUST_FRAME_RATE,
            },
            start_size,
            end_size,
            aspect: Vec3::ONE,
            roll,
            spin,
            erosion_end: GROUND_DUST_EROSION_END,
            rotation: None,
        });
    }
}

/// The source whose share interval holds `draw`, a point in `[0, Σ share)`: the sources laid end
/// to end in slot order. A draw at or past the total (float rounding at the top end) is the last
/// source with a positive share.
fn soil_of(
    sources: &[(f32, crate::ground_types::DustResponse); 4],
    mut draw: f32,
) -> crate::ground_types::DustResponse {
    for (share, soil) in sources {
        if *share > 0.0 && draw < *share {
            return *soil;
        }
        draw -= share;
    }
    sources
        .iter()
        .rev()
        .find(|(share, _)| *share > 0.0)
        .expect("a ground that lifts a puff has a source with a positive share")
        .1
}

/// Decay each muzzle light hard (cubic — most of the drop in the first frames) and despawn at its
/// own lifetime. One system for every gun's lights; the scale rides on the component. A newborn is
/// only ARMED on the first visit, so the frame a light was spawned in renders at its peak whichever
/// side of this system the spawn landed on.
fn decay_muzzle_lights(
    time: Res<Time>,
    mut lights: Query<(Entity, &mut MuzzleLight, &mut PointLight)>,
    roots: Query<Entity>,
    mut cadence: ResMut<SmokeCadence>,
    mut commands: Commands,
) {
    // A despawned source has no future cadence; bound the table to current sources.
    cadence
        .0
        .retain(|source, _| source.is_none_or(|(tank, _)| roots.contains(tank)));
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
    use crate::guns::MuzzleEmission;
    use bevy::render::storage::ShaderBuffer;

    use crate::vfx::billboard::{
        Billboard, BillboardFamilies, VfxBillboardMaterial, age_billboards,
    };

    /// Minimal app carrying what the fire observer and the agers read: bare asset stores, a
    /// fixed-seed view RNG, no camera (distance LOD treats that as near — full dressing), and NO
    /// height grid (so the ground blast finds no known ground; `harness_ground` adds one).
    fn harness() -> App {
        let mut app = App::new();
        app.init_resource::<BillboardRing>()
            .init_resource::<MuzzleLightRing>()
            .init_resource::<SmokeCadence>()
            .init_resource::<DustPalettes>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<VfxBillboardMaterial>>()
            .init_resource::<Assets<ShaderBuffer>>()
            .init_resource::<BillboardFamilies>()
            .init_resource::<Time>()
            .insert_resource(ViewRng::seeded(42))
            .add_observer(dress_fire)
            // BOTH agers: the dressing's contract is what a layer RENDERS after n frames, and the
            // billboard half of it is unreadable without `age_billboards` running.
            .add_systems(Update, (age_billboards, decay_muzzle_lights));
        // The two blast sheets get DISTINCT handles (everything else can share the default one):
        // which sheet a shot picked is only readable off its material.
        let blast_atlas = app
            .world_mut()
            .resource_scope(|_, mut images: Mut<Assets<Image>>| {
                [(); BLAST_SHEETS].map(|()| images.add(Image::default()))
            });
        app.insert_resource(MuzzleVfxAssets {
            renderer: crate::guns::MuzzleLook::shipped().renderer,
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
        });
        app
    }

    /// The harness with a FLAT terrain surface at `height` under it — what the ground blast
    /// measures the muzzle against (the shots below fire from y = 2.0).
    fn harness_ground(height: f32) -> App {
        harness_ground_of(height, 1.0)
    }

    /// [`harness_ground`] painted all base ground of erodibility `erodibility`. FIXTURE DATA: the
    /// shipped table with every ground's erodibility replaced.
    fn harness_ground_of(height: f32, erodibility: f32) -> App {
        let mut app = harness();
        let size = 33usize;
        app.insert_resource(crate::terrain_grid::HeightGrid::new(
            vec![height; size * size].into(),
            size as u32,
            crate::terrain_grid::FIXTURE_EXTENT,
        ));
        let mut table = crate::ground_types::GroundTypes::load_shipped();
        for ground in table.types.values_mut() {
            ground.dust.erodibility = erodibility;
        }
        app.insert_resource(table)
            .insert_resource(crate::track_prints::PrintDepths::unpainted());
        app
    }

    /// The muzzle every test shot leaves from.
    const FIRE_ORIGIN: Vec3 = Vec3::new(1.0, 2.0, 3.0);

    use crate::sim_tick::TickSpan;

    /// FIXTURE DATA: a slow single-shot cycle and a rapid belt; no weapon identity is a rule.
    /// At 64 Hz (DERIVED): a 1 s reload, a 0.078 s cyclic interval and a 1 s swap.
    const SLOW: FireCycle = FireCycle::Single {
        reload: TickSpan::of(64),
    };
    const BELT: FireCycle = FireCycle::Automatic {
        cyclic: TickSpan::of(5),
        belt_size: 100,
        swap: TickSpan::of(64),
        tracer_every: 1,
    };

    /// One shot of `caliber` (m) carrying `muzzle`, from a live weapon on `cycle`.
    fn fire_with(
        app: &mut App,
        muzzle: MuzzleEmission,
        cycle: FireCycle,
        caliber: f32,
        catch_up_ticks: u32,
        tracer: bool,
    ) {
        let root = app
            .world_mut()
            .query::<(&Weapon, &TankRoot)>()
            .iter(app.world())
            .find_map(|(weapon, root)| (weapon.fire_cycle == cycle).then_some(root.0));
        let root = root.unwrap_or_else(|| app.world_mut().spawn_empty().id());
        if app
            .world_mut()
            .query::<(&Weapon, &TankRoot)>()
            .iter(app.world())
            .all(|(_, tank)| tank.0 != root)
        {
            app.world_mut().spawn((
                Weapon {
                    name: "fixture".to_owned(),
                    rounds: Vec::new(),
                    fire_cycle: cycle,
                    recoil: None,
                    barrel: None,
                    fire: Vec::new(),
                    load: Vec::new(),
                    trigger: crate::spec::Trigger::Primary,
                    report_clips: Vec::new(),
                },
                TankRoot(root),
                WeaponIndex(0),
            ));
        }
        app.world_mut().trigger(FireShell {
            effects: crate::rounds::FlightEffects::fixture_traced(),
            muzzle,
            origin: FIRE_ORIGIN,
            direction: Dir3::X,
            speed: 773.0,
            caliber,
            mass: 10.2,
            drag_k: crate::ballistics::fixture_drag_k(caliber, 10.2),
            nose_factor: 0.3,
            penetrator_caliber: caliber,
            penetrator_mass: 10.2,
            wound: 30.0,
            burst: None,
            fuze_mode: 0,
            mechanism: cycle.mechanism(),
            shooter: Some(crate::ballistics::ShotSource {
                tank: root,
                weapon: 0,
                round: 0,
            }),
            tracer,
            shot_origin: FireShellOrigin::Local,
            catch_up_ticks,
            shot: None,
        });
        app.world_mut().flush();
    }

    /// A shot carrying the full fixture dressing.
    fn cannon(app: &mut App, catch_up_ticks: u32) {
        fire_with(
            app,
            MuzzleEmission::fixture_cannon(),
            SLOW,
            CANNON_CALIBER,
            catch_up_ticks,
            true,
        );
    }

    /// A shot carrying the rationed fixture dressing.
    fn rifle(app: &mut App, catch_up_ticks: u32, tracer: bool) {
        fire_with(
            app,
            MuzzleEmission::fixture_rifle(),
            BELT,
            RIFLE_CALIBER,
            catch_up_ticks,
            tracer,
        );
    }

    /// FIXTURE DATA, metres: a large and a small flying body.
    const CANNON_CALIBER: f32 = 0.088;
    const RIFLE_CALIBER: f32 = 0.0079;

    /// The full fixture's airborne layers: core + fireball + glow + 2 forward planes + 2 planes
    /// on each of 2 side jets + smoke.
    const CANNON_AIRBORNE: usize = 10;

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

    /// The game's damage-model view dresses each shot from the emission it carries and never from
    /// its calibre: the full emission on a small body draws every authored layer, the rationed one
    /// on a large body draws only its core and plane, at its own scale. A mutant that reinstates a
    /// calibre split, or sizes a layer off the body, fails here.
    #[test]
    fn each_shot_is_dressed_from_its_emission_never_its_calibre() {
        let mut full = harness();
        fire_with(
            &mut full,
            MuzzleEmission::fixture_cannon(),
            SLOW,
            RIFLE_CALIBER,
            0,
            true,
        );
        assert_eq!(billboards(&mut full), CANNON_AIRBORNE);
        assert_eq!(lights(&mut full), 1);

        let mut rationed = harness();
        fire_with(
            &mut rationed,
            MuzzleEmission::fixture_rifle(),
            BELT,
            CANNON_CALIBER,
            0,
            true,
        );
        assert_eq!(billboards(&mut rationed), 2, "core + 1 flame plane");
        assert_eq!(lights(&mut rationed), 1);
        let largest = MuzzleEmission::fixture_rifle().flames.forward.length_m.1;
        let world = rationed.world_mut();
        for billboard in world.query::<&Billboard>().iter(world) {
            assert!(
                billboard.start_size <= largest,
                "the rationed dressing keeps its own scale (got {} m)",
                billboard.start_size
            );
        }
    }

    /// The game's muzzle rendering: independent taste fields reach the material uniforms.
    /// A hardcoded renderer boost or opacity would prevent the owner's file from controlling it.
    /// FIXTURE DATA: distinct values for each covered layer, unrelated to shipped taste.
    #[test]
    fn sprite_materials_read_the_taste_file() {
        let mut app = harness();
        let mut assets = app.world_mut().resource_mut::<MuzzleVfxAssets>();
        assets.renderer.flash_boost = 9.0;
        assets.renderer.fireball_boost = 3.0;
        assets.renderer.fireball_opacity = 0.42;
        assets.renderer.fireball_sharpness = 0.7;
        assets.renderer.smoke_boost = 1.7;
        assets.renderer.smoke_sharpness = 0.8;
        assets.renderer.dust_sharpness = 0.9;
        let flash = assets.flash_material(Handle::default(), assets.renderer.core_sharpness);
        assert_eq!(flash.params.glow.x, 9.0);
        let blast = assets.blast_material(Handle::default());
        assert_eq!(blast.params.glow.x, 3.0);
        assert_eq!(blast.params.fade.w, 0.42);
        assert_eq!(blast.params.fade.y, 0.7);
        let smoke = assets.smoke_material();
        assert_eq!(smoke.params.glow.x, 1.7);
        assert_eq!(smoke.params.fade.y, 0.8);
        let dust = assets.dust_material(0.5, Handle::default(), 2.0);
        assert_eq!(dust.params.fade.y, 0.9);
    }

    /// The game's damage-model view: every shape's planes point along its requested jet axes,
    /// with exactly one jet on each flank at its resolved angle. Players rely on symmetric gas
    /// routing, independent of taste edits. A duplicated flank or wrong angle fails this test.
    /// FIXTURE DATA: a generic shot and legal forward/backward side-jet angles.
    #[test]
    fn each_muzzle_shape_draws_its_shape_and_only_its_shape() {
        use crate::guns::{Mechanism, MuzzleEnergy, MuzzleLook, MuzzleShape};
        let look = MuzzleLook::shipped();
        let energy = MuzzleEnergy::of(10.0, 780.0).expect("a positive fixture");
        for shape in [MuzzleShape::MuzzleBrake, MuzzleShape::FlashHider] {
            let statement = look.shape(shape);
            for angle in [45.0_f32, 100.0, 135.0] {
                let mut app = harness();
                let mut emission = look.emission(shape, energy, Mechanism::Single).unwrap();
                assert_eq!(
                    emission.flames.side_jets.map(|side| side.angle_rad),
                    statement.side_jets.map(|side| side.angle_rad)
                );
                if let Some(side) = &mut emission.flames.side_jets {
                    side.angle_rad = angle.to_radians();
                }
                fire_with(&mut app, emission, SLOW, CANNON_CALIBER, 0, false);
                let (mut forward, mut left, mut right) = (0u8, 0u8, 0u8);
                let world = app.world_mut();
                for transform in world.query_filtered::<&Transform, (
                    With<Billboard>, Without<crate::vfx::billboard::FaceCamera>,
                )>().iter(world) {
                    // The fixture bore is +X; a flame plane's long axis is rotated +Y.
                    let axis = transform.rotation * Vec3::Y;
                    if axis.dot(Vec3::X) > 0.999 {
                        forward += 1;
                    } else {
                        let side = emission.flames.side_jets.expect("only a stated side jet leaves the bore");
                        assert!((axis.x - side.angle_rad.cos()).abs() < 1e-5, "{shape:?}: {axis:?} at {angle}");
                        assert!(axis.y.abs() < 1e-5);
                        assert!((axis.z.abs() - side.angle_rad.sin()).abs() < 1e-5);
                        if axis.z > 0.0 { left += 1; } else { right += 1; }
                    }
                }
                assert_eq!(forward, statement.forward_planes, "{shape:?}");
                let planes = statement.side_jets.map_or(0, |jets| jets.planes);
                assert_eq!((left, right), (planes, planes), "{shape:?} at {angle}");
                assert_eq!(
                    blast_cores(&mut app).len(),
                    usize::from(statement.fireball),
                    "{shape:?}"
                );
            }
        }
    }

    /// Distance LOD contract: beyond `FAR_FULL_DRESSING` only the core, the fireball and the light
    /// spawn — the glow, flame planes and smoke are near-only.
    #[test]
    fn a_far_shot_drops_to_core_fireball_and_light() {
        let mut app = harness();
        app.world_mut().spawn((
            Camera3d::default(),
            PlayerView,
            GlobalTransform::from_translation(Vec3::new(2000.0, 0.0, 0.0)),
        ));
        cannon(&mut app, 0);
        assert_eq!(billboards(&mut app), 2, "far: fireball + core only");
        assert_eq!(
            lights(&mut app),
            1,
            "the muzzle light carries the read at range"
        );
    }

    /// The ground-dust puffs of the last shot in SPAWN order: `(entity, rendered diameter,
    /// rendered position)` each. The flipbook rate is the discriminator — no other muzzle layer
    /// plays at [`GROUND_DUST_FRAME_RATE`].
    fn dust_puffs(app: &mut App) -> Vec<(Entity, f32, Vec3)> {
        let world = app.world_mut();
        let mut q = world.query::<(Entity, &Billboard, &Transform)>();
        let mut puffs: Vec<(Entity, f32, Vec3)> = q
            .iter(world)
            .filter(|(_, b, _)| {
                matches!(b.flipbook, Flipbook::Loop { rate, .. } if rate == GROUND_DUST_FRAME_RATE)
            })
            .map(|(entity, _, t)| (entity, t.scale.x, t.translation))
            .collect();
        puffs.sort_unstable_by_key(|(entity, _, _)| *entity);
        puffs
    }

    /// The ground blast's reach is the shot's: a muzzle within it over known terrain lifts the
    /// resolved puff count, sitting on that surface; the ground below the reach lifts none, and an
    /// emission without a ground blast lifts none at any height.
    #[test]
    fn ground_dust_needs_the_ground_within_the_shots_reach() {
        let blast = MuzzleEmission::fixture_cannon()
            .ground_blast
            .expect("the full fixture states a ground blast");
        // Muzzle at y = 2.0, ground at y = 0.0: inside the fixture's 2.5 m reach.
        let mut low = harness_ground(0.0);
        cannon(&mut low, 0);
        let puffs = dust_puffs(&mut low);
        assert!(
            (blast.puffs.0 as usize..=blast.puffs.1 as usize).contains(&puffs.len()),
            "dust puff count {} outside {:?}",
            puffs.len(),
            blast.puffs
        );
        assert_eq!(billboards(&mut low), CANNON_AIRBORNE + puffs.len());
        for (_, size, position) in &puffs {
            assert!(
                position.y >= 0.0 && position.y <= size * 0.5 + 1e-3,
                "a puff must hug the ground it was lifted from (y = {})",
                position.y
            );
        }

        let mut high = harness_ground(-6.0);
        cannon(&mut high, 0);
        assert!(
            dust_puffs(&mut high).is_empty(),
            "beyond the reach lifts none"
        );

        let mut none = harness_ground(0.0);
        rifle(&mut none, 0, true);
        assert!(
            dust_puffs(&mut none).is_empty(),
            "no authored blast, no dust"
        );
    }

    /// The ground's own response decides how much of the shot's cloud it gives up: fully erodible
    /// ground lifts the rolled count, half-erodible ground half of it (rounded), and a nonerodible
    /// ground under the same muzzle lifts nothing. A world without a drawn ground states no
    /// erodibility and lifts nothing either. A mutant that ignores the ground fails.
    #[test]
    fn ground_dust_follows_the_grounds_erodibility() {
        let count = |erodibility: f32| {
            let mut app = harness_ground_of(0.0, erodibility);
            cannon(&mut app, 0);
            dust_puffs(&mut app).len()
        };
        let full = count(1.0);
        assert!(full > 0);
        assert_eq!(count(0.5), (full as f32 * 0.5).round() as usize);
        assert_eq!(count(0.0), 0);

        let mut undrawn = harness();
        let size = 33usize;
        undrawn.insert_resource(crate::terrain_grid::HeightGrid::new(
            vec![0.0; size * size].into(),
            size as u32,
            crate::terrain_grid::FIXTURE_EXTENT,
        ));
        cannon(&mut undrawn, 0);
        assert!(dust_puffs(&mut undrawn).is_empty());
    }

    /// The sunlit (young, full-signal) texel of a dust puff's palette, linear 8-bit RGB, and the
    /// light its material carries (`1 + glow.x`, the palette's gain lane being one).
    fn puff_soil(app: &mut App, puff: Entity) -> ([u8; 3], f32) {
        let world = app.world();
        let material = world
            .get::<MeshMaterial3d<VfxBillboardMaterial>>(puff)
            .expect("a puff draws a billboard material");
        let lut = world
            .resource::<Assets<VfxBillboardMaterial>>()
            .get(&material.0)
            .expect("the family material is stored")
            .lut
            .clone();
        let data = world
            .resource::<Assets<Image>>()
            .get(&lut)
            .expect("the palette is stored")
            .data
            .as_ref()
            .expect("the palette carries its texels");
        // The palette is 64 × 16 RGBA8: x = 63 is full signal, row 0 the newborn cloud.
        let texel = 63 * 4;
        let light = world
            .get::<Billboard>(puff)
            .expect("a billboard")
            .lanes
            .glow
            .x
            + 1.0;
        ([data[texel], data[texel + 1], data[texel + 2]], light)
    }

    /// The lit soil texel: linear albedo times the world key light's linear colour, in 8-bit RGB.
    fn stored_linear(colour_srgb: [f32; 3]) -> [u8; 3] {
        let albedo = LinearRgba::from(Color::srgb_from_array(colour_srgb));
        let sun = crate::world::sun_light().color.to_linear();
        [
            albedo.red * sun.red,
            albedo.green * sun.green,
            albedo.blue * sun.blue,
        ]
        .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
    }

    /// Lifted soil is the ground's own, lit by the key light like the ground it rose from: every
    /// puff's sunlit texel is the lifting ground's albedo times the sun's linear colour;
    /// its material carries the sun's
    /// radiance per unit albedo on level ground under the default exposure. Two grounds differing
    /// only in colour lift differently coloured clouds. A mutant that keeps one shared earth
    /// palette, drops the sun's colour, or draws the dust unlit, fails. FIXTURE DATA: the shipped table with the base
    /// ground's colour replaced.
    #[test]
    fn each_puff_is_its_grounds_soil_lit_by_the_key_light() {
        let light = crate::world::level_ground_irradiance_lux() / std::f32::consts::PI
            * Exposure::default().exposure();
        for colour in [[0.6, 0.5, 0.3], [0.2, 0.25, 0.3]] {
            let mut app = harness_ground(0.0);
            app.world_mut()
                .resource_mut::<crate::ground_types::GroundTypes>()
                .types
                .get_mut(crate::terrain_blend::BASE_GROUND)
                .expect("the base pack's ground")
                .dust
                .colour_srgb = colour;
            cannon(&mut app, 0);
            let puffs = dust_puffs(&mut app);
            assert!(!puffs.is_empty());
            for (puff, _, _) in puffs {
                let (texel, lit) = puff_soil(&mut app, puff);
                assert_eq!(
                    texel,
                    stored_linear(colour),
                    "the puff is the ground's soil"
                );
                assert!(
                    (lit - light).abs() < 1e-4,
                    "lit at {lit}, the key light is {light}"
                );
            }
        }
    }

    /// A puff's soil is drawn by share from the grounds under the muzzle: the sources lie end to
    /// end in slot order, a zero share is never drawn, and a draw rounding onto the total lands
    /// on the last lifting source. A mutant that ignores the shares draws the wrong soil.
    #[test]
    fn a_puffs_soil_is_drawn_by_its_grounds_share() {
        let soil = |red: f32| crate::ground_types::DustResponse {
            erodibility: 1.0,
            colour_srgb: [red, 0.0, 0.0],
        };
        // FIXTURE DATA: four distinguishable soils, the first and third lifting nothing.
        let sources = [
            (0.0, soil(0.1)),
            (0.25, soil(0.2)),
            (0.0, soil(0.3)),
            (0.5, soil(0.4)),
        ];
        let red = |draw: f32| soil_of(&sources, draw).colour_srgb[0];
        assert_eq!(red(0.0), 0.2);
        assert_eq!(red(0.249), 0.2);
        assert_eq!(red(0.25), 0.4);
        assert_eq!(red(0.749), 0.4);
        assert_eq!(red(0.75), 0.4);
    }

    /// MEASURED, `--nocapture`: each shipped shot's ground blast on each shipped ground, the base
    /// slot standing in for that ground. Class invariants only: a ground lifts the shot's rolled
    /// range scaled by its erodibility, every puff of its own soil, at the shot's metres and life.
    #[test]
    fn every_shipped_blast_lifts_each_grounds_own_soil() {
        const SHOTS: usize = 24;
        let shipped = crate::ground_types::GroundTypes::load_shipped();
        let blasts: Vec<(&str, &str, MuzzleEmission)> = crate::guns::GunRegistry::shipped()
            .fired_rounds()
            .filter(|(_, _, round)| round.muzzle.ground_blast.is_some())
            .map(|(gun, key, round)| (gun, key, round.muzzle))
            .collect();
        assert!(
            !blasts.is_empty(),
            "no shipped gun lifts dust: the survey would be vacuous"
        );
        for (gun, round, muzzle) in blasts {
            let blast = muzzle.ground_blast.expect("filtered on a blast");
            for (name, ground) in &shipped.types {
                let erodibility = ground.dust.erodibility;
                let range = (
                    (blast.puffs.0 as f32 * erodibility).round() as usize,
                    (blast.puffs.1 as f32 * erodibility).round() as usize,
                );
                let (mut least, mut most) = (usize::MAX, 0);
                let mut ends = (f32::MAX, 0.0_f32);
                let mut lives = (f32::MAX, 0.0_f32);
                for shot in 0..SHOTS {
                    // One shot per app, each under its own view seed: the rolls differ, and no
                    // ring eviction can recycle a puff's id between shots.
                    // The muzzle stands half this shot's reach over the ground: inside it whatever
                    // the shot's energy.
                    let mut app = harness_ground(FIRE_ORIGIN.y - 0.5 * blast.reach_m);
                    app.insert_resource(ViewRng::seeded(shot as u64));
                    app.world_mut()
                        .resource_mut::<crate::ground_types::GroundTypes>()
                        .types
                        .insert(crate::terrain_blend::BASE_GROUND.to_owned(), *ground);
                    fire_with(&mut app, muzzle, SLOW, CANNON_CALIBER, 0, true);
                    let puffs = dust_puffs(&mut app);
                    let count = puffs.len();
                    assert!(
                        (range.0..=range.1).contains(&count),
                        "{gun}/{round} on {name}: {count} puffs outside {range:?}"
                    );
                    for &(puff, size, _) in &puffs {
                        let soil = puff_soil(&mut app, puff).0;
                        assert_eq!(
                            soil,
                            stored_linear(ground.dust.colour_srgb),
                            "{gun}/{round} on {name}"
                        );
                        assert!((size - blast.birth_diameter_m).abs() < 1e-4);
                        let billboard = app.world().get::<Billboard>(puff).expect("a billboard");
                        let end = billboard.end_size;
                        let lifetime = billboard.lifetime;
                        assert!(
                            (blast.end_diameter_m.0..=blast.end_diameter_m.1).contains(&end),
                            "{gun}/{round} on {name}: spawned diameter {end} outside {:?}",
                            blast.end_diameter_m
                        );
                        assert!(
                            (blast.lifetime_s.0..=blast.lifetime_s.1).contains(&lifetime),
                            "{gun}/{round} on {name}: spawned lifetime {lifetime} outside {:?}",
                            blast.lifetime_s
                        );
                        ends = (ends.0.min(end), ends.1.max(end));
                        lives = (lives.0.min(lifetime), lives.1.max(lifetime));
                    }
                    (least, most) = (least.min(count), most.max(count));
                }
                eprintln!(
                    "MEASURED {gun}/{round} on {name} (erodibility {erodibility}, soil {:?}): {least}-{most} \
                     puffs over {SHOTS} shots, {:.2} m at birth to {:.2}-{:.2} m, {:.3}-{:.3} s",
                    ground.dust.colour_srgb,
                    blast.birth_diameter_m,
                    ends.0,
                    ends.1,
                    lives.0,
                    lives.1
                );
            }
        }
    }

    /// The cloud's metres are the emission's (ADR-0023: physical inputs only, never the viewer): a
    /// puff is born at the resolved diameter and lifts at the resolved rise; doubling both in the
    /// emission doubles both on screen. A mutant reading a shared constant or the calibre fails.
    #[test]
    fn ground_dust_reads_the_resolved_cloud() {
        const LIFT_SECS: f32 = 1.0;
        fn puff(muzzle: MuzzleEmission) -> (f32, f32) {
            let mut app = harness_ground(0.0);
            fire_with(&mut app, muzzle, SLOW, CANNON_CALIBER, 0, true);
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
        let authored = MuzzleEmission::fixture_cannon();
        let blast = authored.ground_blast.expect("fixture blast");
        let (size, lift) = puff(authored);
        assert!((size - blast.birth_diameter_m).abs() < 1e-4);
        assert!(
            (lift - blast.rise_mps * LIFT_SECS).abs() < 1e-4,
            "rose {lift} m"
        );

        let mut doubled = authored;
        doubled.ground_blast = Some(GroundBlast {
            birth_diameter_m: blast.birth_diameter_m * 2.0,
            rise_mps: blast.rise_mps * 2.0,
            ..blast
        });
        let (big, big_lift) = puff(doubled);
        assert!((big - size * 2.0).abs() < 1e-4, "{size} → {big}");
        assert!((big_lift - lift * 2.0).abs() < 1e-4, "{lift} → {big_lift}");
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

    /// The SHIPPED sheets, read off disk: [`BLAST_COLS`]×[`BLAST_ROWS`] cells of the size
    /// `scripts/vfx/blast_atlas.py` authors, and a mip chain — the grid constants above are what
    /// the shader divides the atlas by, and the sheet is what it divides, so nothing else in the
    /// suite can catch the two disagreeing. The chain is the whole reason these two sprites are
    /// KTX2: a mipless blast shimmers at the ranges it is drawn at.
    #[test]
    fn the_shipped_blast_sheets_are_the_grid_the_shader_divides() {
        /// Output cell size in texels (`CELL_PX` in `scripts/vfx/blast_atlas.py`).
        const CELL_PX: u32 = 256;
        for sheet in ["blast_core_a.ktx2", "blast_core_b.ktx2"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/vfx")
                .join(sheet);
            let bytes = std::fs::read(&path).unwrap_or_else(|err| panic!("{sheet}: {err}"));
            // KTX2 header (§3.1): a 12-byte identifier, then u32 LE fields — pixelWidth at 20,
            // pixelHeight at 24, levelCount at 40.
            let field = |offset: usize| {
                u32::from_le_bytes(
                    bytes[offset..offset + 4]
                        .try_into()
                        .unwrap_or_else(|_| panic!("{sheet} is truncated")),
                )
            };
            assert_eq!(&bytes[1..7], b"KTX 20", "{sheet} is not a KTX2 file");
            assert_eq!(
                (field(20), field(24)),
                (BLAST_COLS as u32 * CELL_PX, BLAST_ROWS as u32 * CELL_PX),
                "{sheet} is not the grid the blast material divides it by"
            );
            assert!(
                field(40) > 1,
                "{sheet} carries no mip chain ({} level)",
                field(40)
            );
        }
    }

    /// The fireball plays its resolved geometry: a diameter inside the resolved range, the
    /// resolved standoff and lifetime, once through the sheet. An emission stating no fireball
    /// draws none.
    #[test]
    fn the_fireball_plays_its_resolved_geometry() {
        let fireball = MuzzleEmission::fixture_cannon()
            .fireball
            .expect("the full fixture states a fireball");
        let mut app = harness();
        cannon(&mut app, 0);
        let entity = blast_cores(&mut app)[0];
        let core = app.world().get::<Billboard>(entity).expect("fireball");
        assert_eq!(
            core.flipbook,
            Flipbook::Once,
            "the sheet is a sequence, not a loop"
        );
        assert!((fireball.diameter_m.0..=fireball.diameter_m.1).contains(&core.start_size));
        assert!((core.lifetime - fireball.lifetime_s).abs() < 1e-6);
        assert!((core.origin.distance(FIRE_ORIGIN) - fireball.standoff_m).abs() < 1e-4);

        let mut none = harness();
        rifle(&mut none, 0, true);
        assert!(blast_cores(&mut none).is_empty());
    }

    /// The game's damage-model view applies the shared light rule to every shot. FIXTURE DATA: the
    /// slower single-shot cycle casts and the short rapid-belt flash does not; a caller that
    /// hardcodes its shadow decision fails the cadence/duration cases below.
    #[test]
    fn every_shot_uses_its_loaded_flash_cadence() {
        let mut slow = harness();
        cannon(&mut slow, 0);
        let world = slow.world_mut();
        let mut q = world.query::<&PointLight>();
        assert!(q.single(world).expect("one light").shadow_maps_enabled);

        let mut belt = harness();
        rifle(&mut belt, 0, true);
        let world = belt.world_mut();
        let mut q = world.query::<&PointLight>();
        assert!(!q.single(world).expect("one light").shadow_maps_enabled);
    }

    /// The game's damage-model view: the muzzle light glows at the emission's peak whether or
    /// not the shot carries a tracer, so the muzzle energy stays its one scalar. A mutant that
    /// brightens a tracer's flash fails.
    #[test]
    fn a_tracer_leaves_the_muzzle_light_at_its_emission_peak() {
        let light = MuzzleEmission::fixture_rifle().light;
        for tracer in [false, true] {
            let mut app = harness();
            rifle(&mut app, 0, tracer);
            let world = app.world_mut();
            let point = world
                .query::<&PointLight>()
                .single(world)
                .expect("one light");
            assert_eq!(point.intensity, light.peak_lm, "tracer {tracer}");
        }
    }

    /// The game's damage-model fire cause can outlive its firing source. The view must still
    /// draw that isolated flash; a live-weapon expect mutant panics on this delivered event.
    #[test]
    fn a_delivered_flash_may_outlive_its_firing_source() {
        let mut app = harness();
        let source = app.world_mut().spawn_empty().id();
        // FIXTURE DATA: a round from the first loaded gun, without assuming unique chambering.
        let guns = crate::guns::GunRegistry::shipped();
        let gun = crate::armaments_index::GUN_FILES[0].0;
        let round = crate::rounds::RoundRegistry::shipped()
            .keys()
            .find_map(|key| guns.round(gun, key).ok())
            .expect("a loaded gun chambers at least one registered round");
        let fire = FireShell::of_round(
            &round,
            FIRE_ORIGIN,
            Dir3::X,
            crate::spec::FireMechanism::Single,
            true,
            Some(crate::ballistics::ShotSource {
                tank: source,
                weapon: 0,
                round: 0,
            }),
            None,
        );
        assert!(app.world_mut().despawn(source));
        app.world_mut().trigger(fire);
        app.world_mut().flush();
        let world = app.world_mut();
        let light = world
            .query::<&PointLight>()
            .single(world)
            .expect("the isolated flash");
        assert!(light.shadow_maps_enabled);
    }

    /// The game's damage-model view shadows persistent or isolated lights, independent of
    /// weapon class. FIXTURE DATA spans short/long lives and fast/slow/absent refire intervals.
    /// A mutant that ignores either lifetime or cadence breaks this presentation contract.
    #[test]
    fn shadow_casting_follows_light_lifetime_and_refire_interval() {
        for (lifetime, interval, casts) in [
            (0.05, Some(0.08), false),
            (0.05, Some(1.0), true),
            (0.2, Some(0.08), true),
            (0.05, None, true),
        ] {
            let mut world = World::new();
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, &world);
            let light = crate::guns::FlashLight {
                lifetime_s: lifetime,
                ..MuzzleEmission::fixture_rifle().light
            };
            spawn_muzzle_light(
                &mut commands,
                &mut MuzzleLightRing::default(),
                Vec3::ZERO,
                &light,
                interval,
            );
            queue.apply(&mut world);
            let light = world.query::<&PointLight>().single(&world).unwrap();
            assert_eq!(
                light.shadow_maps_enabled, casts,
                "life {lifetime}, interval {interval:?}"
            );
        }
    }

    /// A rationed smoke puffs every `every_rounds`-th shot — per-round puffs at the cyclic rate
    /// are the overdraw trap.
    #[test]
    fn smoke_spawns_on_the_guns_ration() {
        let every = MuzzleEmission::fixture_rifle()
            .smoke
            .expect("the rationed fixture smokes")
            .every_rounds;
        let mut app = harness();
        let rounds = every * 2;
        for _ in 0..rounds {
            rifle(&mut app, 0, false);
        }
        // Each round spawns core + flame plane; every Nth adds one puff.
        let expected = (rounds * 2 + rounds / every) as usize;
        assert_eq!(
            billboards(&mut app),
            expected,
            "2/round + 1 puff per {every}"
        );
    }

    /// The game's damage-model view rations smoke per mount. Interleaved fire from another
    /// mount or tank must not advance it, and changing ammunition in the same mount must not
    /// restart it. The renderer relies on exactly the authored share per source.
    #[test]
    fn smoke_rations_are_independent_of_other_sources_and_round_selection() {
        use crate::ballistics::ShotSource;
        let mut world = World::new();
        let tank = world.spawn_empty().id();
        let other = world.spawn_empty().id();
        let source = ShotSource {
            tank,
            weapon: 0,
            round: 0,
        };
        let mut cadence = SmokeCadence::default();
        assert!(!cadence.puff(Some(source), 3));
        for other in [
            ShotSource {
                weapon: 1,
                ..source
            },
            ShotSource {
                tank: other,
                ..source
            },
        ] {
            assert!(!cadence.puff(Some(other), 3));
            assert!(!cadence.puff(Some(other), 3));
            assert!(cadence.puff(Some(other), 3));
        }
        assert!(!cadence.puff(Some(ShotSource { round: 1, ..source }), 3));
        assert!(cadence.puff(Some(source), 3));
        assert!(!cadence.puff(Some(source), 3));
    }

    /// A stale remote shot (catch-up beyond ~250 ms) skips the dressing rather than playing late.
    #[test]
    fn stale_remote_fire_skips_dressing() {
        let mut app = harness();
        cannon(&mut app, STALE_FIRE_TICKS + 1);
        assert_eq!(billboards(&mut app), 0);
        assert_eq!(lights(&mut app), 0);
        rifle(&mut app, STALE_FIRE_TICKS + 1, true);
        assert_eq!(billboards(&mut app), 0, "stale burst skips too");
        assert_eq!(lights(&mut app), 0);
        // At or under the boundary the dressing still plays (~150 ms catch-up is the normal case).
        cannon(&mut app, STALE_FIRE_TICKS);
        assert_eq!(billboards(&mut app), CANNON_AIRBORNE);
    }

    /// The muzzle light decays monotonically from its first-frame peak and despawns at end of
    /// life — the "first frame hottest" contract, which holds through a whole rendered frame.
    #[test]
    fn muzzle_light_decays_then_despawns() {
        let light = MuzzleEmission::fixture_cannon().light;
        let mut app = harness();
        cannon(&mut app, 0);
        let world = app.world_mut();
        let mut q = world.query::<(&MuzzleLight, &PointLight)>();
        let (_, point) = q.single(world).expect("one light");
        assert_eq!(point.intensity, light.peak_lm, "born at peak");

        // Frame one at 60 fps — and it is still at peak.
        advance(&mut app, 1.0 / 60.0);
        let world = app.world_mut();
        let mut q = world.query::<&PointLight>();
        assert_eq!(
            q.single(world).expect("alive on frame one").intensity,
            light.peak_lm,
            "the first RENDERED frame is the peak, not one frame down the decay"
        );

        advance(&mut app, light.lifetime_s * 0.5);
        let world = app.world_mut();
        let mut q = world.query::<(&MuzzleLight, &PointLight)>();
        let (_, point) = q.single(world).expect("still alive mid-decay");
        assert!(
            point.intensity < light.peak_lm * 0.5,
            "cubic decay front-loads the drop"
        );

        advance(&mut app, light.lifetime_s);
        assert_eq!(lights(&mut app), 0, "expired light must despawn");
    }

    /// Refire storms are bounded: the light ring evicts oldest-first at its cap (the billboard
    /// ring's own cap is pinned in the billboard tests).
    #[test]
    fn light_ring_caps_refire() {
        let mut app = harness();
        let mut spawned = Vec::with_capacity(LIGHT_CAP + 5);
        for _ in 0..LIGHT_CAP + 5 {
            cannon(&mut app, 0);
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
