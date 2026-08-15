//! View-layer audio: the trigger's click, the main gun's report, and the per-tank engine loop.
//!
//! Invariant (ADR-0014): these systems subscribe to the input and simulation seams and write no
//! simulation state; the report's variant roll is view-local randomness. Mounted by the windowed
//! client compositions only — the dedicated server runs without an audio device.
//!
//! Every world-placed emitter is spatial and shares one falloff law ([`SPATIAL_SCALE`]); the
//! listener rides the one 3-D camera (`camera::spawn_camera`).

use bevy::audio::{DefaultSpatialScale, SpatialScale, Volume};
use bevy::prelude::*;

use crate::ballistics::{FireShell, STALE_FIRE_TICKS, TRACER_MAX_CALIBER};
use crate::command::Bindings;
use crate::state::{GameplaySet, PlayerInputSet};
use crate::track::sim::TankTransmission;
use crate::track::transmission::RPM_TO_RAD;
use crate::vfx::ViewRng;

/// Spatial falloff scale. Bevy's attenuation is `1/(distance × scale)`, so this puts unity gain at
/// 50 m and overrides `AudioPlugin`'s default of 1.0 (unity at 1 m — battlefield distances would
/// arrive inaudible).
const SPATIAL_SCALE: f32 = 0.02;

/// Listener ear separation (m) — the stereo panning width, not a head measurement. Bevy's default
/// is 4 m, which pans anything closer than that hard to one side.
pub(crate) const LISTENER_EAR_GAP: f32 = 0.2;

/// Emitter volumes. The click sits under the world sounds (it is a switch, not an event in the
/// world); the engine sits under both, since it plays continuously on every tank in earshot.
const CLICK_VOLUME: f32 = 0.5;
const CANNON_VOLUME: f32 = 1.0;
const ENGINE_VOLUME: f32 = 0.35;

/// The trigger click, non-spatial (it happens at the player's own hand).
const CLICK_CLIP: &str = "sfx/click/click_1.ogg";
/// The engine loop — a seamless 3.0 s sample, played at [`engine_playback_speed`].
const ENGINE_CLIP: &str = "sfx/engine/engine_loop_44hz.ogg";
/// The main gun's report variants, one rolled per shot.
const CANNON_CLIPS: &[&str] = &[
    "sfx/explosion/88_cannon_1.ogg",
    "sfx/explosion/88_cannon_2.ogg",
    "sfx/explosion/88_cannon_3.ogg",
    "sfx/explosion/88_cannon_4.ogg",
    "sfx/explosion/88_cannon_5.ogg",
];

/// The engine sample's own cylinder-pop rate (Hz), MEASURED off the shipped loop — the filename's
/// number is not it.
const ENGINE_LOOP_POP_HZ: f32 = 44.3;
/// The rpm the sample is anchored at (the Tiger's idle).
const ENGINE_ANCHOR_RPM: f32 = 600.0;
/// Cylinder-firing rate (Hz) at [`ENGINE_ANCHOR_RPM`] for a twelve-cylinder four-stroke:
/// `rpm/60 × 12/2`.
const ENGINE_ANCHOR_POP_HZ: f32 = 60.0;
/// Playback speed that lands the sample's pops on [`ENGINE_ANCHOR_POP_HZ`].
const ENGINE_ANCHOR_SPEED: f32 = ENGINE_ANCHOR_POP_HZ / ENGINE_LOOP_POP_HZ;
/// Compression exponent on the rpm ratio. Rate-honest tracking is exponent 1: idle→governed is
/// 2500/600 = ×4.167, which tape-shifts the sample to ×5.64. `ln(2)/ln(4.167)` compresses that span
/// to one octave instead, putting the governed 2500 rpm at ×2.70.
const ENGINE_SPEED_EXPONENT: f32 = 0.483;
/// Playback-speed bounds: the floor holds a stalling engine off the sub-bass, the ceiling caps the
/// tape shift an over-revved crank could reach.
const ENGINE_SPEED_MIN: f32 = 0.9;
const ENGINE_SPEED_MAX: f32 = 2.8;
/// Time constant (s) of the exponential approach toward the mapped speed — the sim's crank speed
/// steps at the fixed rate, and an unsmoothed `set_speed` per frame reads as a warble.
const ENGINE_SPEED_TAU: f32 = 0.12;

pub fn plugin(app: &mut App) {
    app.init_resource::<ViewRng>()
        .init_resource::<LastCannonVariant>()
        // The one falloff law for every spatial emitter in the game.
        .insert_resource(DefaultSpatialScale(SpatialScale::new(SPATIAL_SCALE)))
        .add_systems(Startup, setup_sfx_assets)
        .add_observer(on_main_gun_report)
        .add_systems(
            Update,
            // Gated exactly as `command::gather_commands` is: a click with the cursor released is
            // menu input, not a trigger pull.
            play_fire_click.in_set(PlayerInputSet).in_set(GameplaySet),
        )
        // Attach before drive so a tank that appears this frame is already carrying its emitter.
        .add_systems(Update, (attach_engine_loops, drive_engine_speed).chain());
    // Dev-only guard: confirm every clip path resolves, so a renamed or missing file surfaces as a
    // loud error instead of silence.
    #[cfg(debug_assertions)]
    app.add_systems(Update, verify_sfx_assets);
}

/// Every clip the sfx layer plays, loaded once at Startup.
#[derive(Resource)]
struct SfxAssets {
    click: Handle<AudioSource>,
    cannon: Vec<Handle<AudioSource>>,
    engine_loop: Handle<AudioSource>,
}

fn setup_sfx_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(SfxAssets {
        click: asset_server.load(CLICK_CLIP),
        cannon: CANNON_CLIPS
            .iter()
            .map(|path| asset_server.load(*path))
            .collect(),
        engine_loop: asset_server.load(ENGINE_CLIP),
    });
}

/// Dev-time asset-load guard: each frame until every clip has settled, check its load state and
/// `error!` any that FAILED (a bad path, a missing file, a codec bevy was not built with — only
/// Vorbis decodes here). Cheap — the paths are already loaded by [`setup_sfx_assets`], so
/// `asset_server.load` returns the existing handle, and the system idles off once everything is
/// settled. `debug_assertions` only, so shipped clients never pay for it.
#[cfg(debug_assertions)]
fn verify_sfx_assets(asset_server: Res<AssetServer>, mut done: Local<bool>) {
    use bevy::asset::LoadState;

    if *done {
        return;
    }
    let mut all_settled = true;
    for path in [CLICK_CLIP, ENGINE_CLIP].iter().chain(CANNON_CLIPS) {
        let handle: Handle<AudioSource> = asset_server.load(*path);
        match asset_server.load_state(&handle) {
            LoadState::Failed(err) => {
                error!("sfx asset failed to load: {path}: {err}");
            }
            LoadState::Loaded => {}
            // NotLoaded / Loading: come back next frame.
            _ => all_settled = false,
        }
    }
    if all_settled {
        *done = true;
    }
}

/// The trigger's own click, straight off the device edge. `TankCommand::fire_primary` is a latched
/// edge a fixed tick consumes, so reading it here would click again on every frame the latch
/// survives — this reads the binding exactly as `command::gather_commands` does. It plays on every
/// physical press: the click is the switch, and the reload gate belongs to the gun.
fn play_fire_click(
    bindings: Res<Bindings>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    assets: Res<SfxAssets>,
    mut commands: Commands,
) {
    if !bindings.fire_primary.just_pressed(&keys, &mouse) {
        return;
    }
    commands.spawn((
        AudioPlayer::new(assets.click.clone()),
        PlaybackSettings::DESPAWN.with_volume(Volume::Linear(CLICK_VOLUME)),
    ));
}

/// The variant played by the last main-gun report — the one-deep memory the re-roll below tests
/// against, so the same clip never lands twice in a row on the first roll.
#[derive(Resource, Default)]
struct LastCannonVariant(Option<usize>);

/// The main gun's report, on the same `FireShell` seam the flash rides. MG-calibre rounds pass
/// through untouched — they have no clip in this slice.
fn on_main_gun_report(
    fire: On<FireShell>,
    assets: Res<SfxAssets>,
    mut rng: ResMut<ViewRng>,
    mut last: ResMut<LastCannonVariant>,
    mut commands: Commands,
) {
    // The same boundary as `vfx::muzzle`'s dressing: this report is the main gun's.
    if fire.caliber < TRACER_MAX_CALIBER {
        return;
    }
    // Stale remote shot: the report's moment is long past — skip, don't play late.
    if fire.catch_up_ticks > STALE_FIRE_TICKS {
        return;
    }
    let mut variant = roll_variant(&mut rng, assets.cannon.len());
    // One re-roll on an immediate repeat; the second roll stands whatever it lands on.
    if last.0 == Some(variant) {
        variant = roll_variant(&mut rng, assets.cannon.len());
    }
    last.0 = Some(variant);
    commands.spawn((
        AudioPlayer::new(assets.cannon[variant].clone()),
        PlaybackSettings::DESPAWN
            .with_spatial(true)
            .with_volume(Volume::Linear(CANNON_VOLUME)),
        Transform::from_translation(fire.origin),
    ));
}

/// One uniform index in `0..count`; `count` is never zero (the clip list is a literal).
fn roll_variant(rng: &mut ViewRng, count: usize) -> usize {
    rng.range(0.0, count as f32).floor() as usize
}

/// Marks a tank whose engine emitter has been attached.
#[derive(Component)]
struct EngineSfx;

/// The looping engine emitter itself — a child of the tank root, so it rides the presented pose and
/// dies with the body.
#[derive(Component)]
struct EngineEmitter;

/// Give every tank carrying a drivetrain a looping engine emitter. `TankTransmission` is replicated
/// to remotes too, so opponents idle and rev on the same code path.
fn attach_engine_loops(
    assets: Res<SfxAssets>,
    tanks: Query<(Entity, &TankTransmission), Without<EngineSfx>>,
    mut commands: Commands,
) {
    for (tank, transmission) in &tanks {
        commands.entity(tank).insert(EngineSfx);
        commands.spawn((
            EngineEmitter,
            AudioPlayer::new(assets.engine_loop.clone()),
            PlaybackSettings::LOOP
                .with_spatial(true)
                .with_volume(Volume::Linear(ENGINE_VOLUME))
                // Born at the crank's current speed, so a tank spawned under load does not open
                // with a frame of idle.
                .with_speed(engine_playback_speed(transmission.0.omega_e / RPM_TO_RAD)),
            Transform::default(),
            ChildOf(tank),
        ));
    }
}

/// Ride each emitter's playback speed toward its tank's crank speed. The sink component appears
/// only once bevy has started the source, so an emitter whose clip is still loading simply does not
/// match this query.
fn drive_engine_speed(
    time: Res<Time>,
    tanks: Query<&TankTransmission>,
    emitters: Query<(&ChildOf, &SpatialAudioSink), With<EngineEmitter>>,
) {
    // Frame-rate-independent exponential approach over ENGINE_SPEED_TAU.
    let blend = 1.0 - (-time.delta_secs() / ENGINE_SPEED_TAU).exp();
    for (parent, sink) in &emitters {
        let Ok(transmission) = tanks.get(parent.parent()) else {
            continue;
        };
        let target = engine_playback_speed(transmission.0.omega_e / RPM_TO_RAD);
        let current = sink.speed();
        sink.set_speed(current + (target - current) * blend);
    }
}

/// Playback speed for the engine loop at `rpm`: the anchor speed ([`ENGINE_ANCHOR_SPEED`], which
/// lands the sample's measured pop rate on the firing rate at [`ENGINE_ANCHOR_RPM`]) scaled by the
/// rpm ratio compressed through [`ENGINE_SPEED_EXPONENT`], clamped to the playable band.
fn engine_playback_speed(rpm: f32) -> f32 {
    let ratio = (rpm / ENGINE_ANCHOR_RPM).max(0.0);
    (ENGINE_ANCHOR_SPEED * ratio.powf(ENGINE_SPEED_EXPONENT))
        .clamp(ENGINE_SPEED_MIN, ENGINE_SPEED_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Tiger's governed crank speed — the top of the band the mapping is shaped over.
    const GOVERNED_RPM: f32 = 2500.0;

    #[test]
    fn engine_speed_anchors_the_sample_rate_at_idle() {
        // 60 Hz of firing over a 44.3 Hz sample.
        assert!((engine_playback_speed(ENGINE_ANCHOR_RPM) - 1.354).abs() < 1e-3);
    }

    #[test]
    fn engine_speed_holds_the_governed_crank_inside_the_band() {
        let governed = engine_playback_speed(GOVERNED_RPM);
        assert!((governed - 2.698).abs() < 1e-3, "governed speed {governed}");
        assert!(governed < ENGINE_SPEED_MAX);
    }

    #[test]
    fn engine_speed_clamps_outside_the_band() {
        assert_eq!(engine_playback_speed(0.0), ENGINE_SPEED_MIN);
        assert_eq!(engine_playback_speed(-100.0), ENGINE_SPEED_MIN);
        assert_eq!(engine_playback_speed(20_000.0), ENGINE_SPEED_MAX);
    }

    #[test]
    fn engine_speed_rises_with_rpm() {
        let mut previous = engine_playback_speed(ENGINE_ANCHOR_RPM);
        for step in 1..=19 {
            let rpm = ENGINE_ANCHOR_RPM + step as f32 * 100.0;
            let speed = engine_playback_speed(rpm);
            assert!(speed > previous, "speed fell at {rpm} rpm: {speed}");
            previous = speed;
        }
    }
}
