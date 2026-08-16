//! View-layer audio: the trigger's click, each weapon's report, and the per-tank engine loop.
//!
//! Invariant (ADR-0014): these systems subscribe to the input and simulation seams and write no
//! simulation state; the report's variant roll is view-local randomness. Mounted by the windowed
//! client compositions only — the dedicated server runs without an audio device.
//!
//! Every world-placed emitter is spatial and shares one falloff law ([`SPATIAL_SCALE`]); the
//! listener rides the one 3-D camera (`camera::spawn_camera`).
//!
//! No tank fact lives here. Clips, the recording's measured pop rate, cylinder count and the crank
//! band are all authored in `<tank>.tank.ron` and reach this layer on the spec-derived components
//! ([`Weapon::report_clips`], [`EngineSound`]); what stays is law — the four-stroke firing rate, the
//! one-octave band, the exponential slew — plus the presentation constants (volumes, falloff) that
//! are properties of the mix rather than of any vehicle.

use std::collections::HashMap;

use bevy::audio::{DefaultSpatialScale, SpatialScale, Volume};
use bevy::prelude::*;

use crate::ballistics::{FireShell, STALE_FIRE_TICKS};
use crate::command::Bindings;
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{EngineSound, Muzzle, TankRoot, Weapon, WeaponIndex};
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
const REPORT_VOLUME: f32 = 1.0;
const ENGINE_VOLUME: f32 = 0.35;

/// The trigger click, non-spatial (it happens at the player's own hand). The one fixed clip path in
/// this layer: it is player UI feedback for a device edge, not a sound any vehicle makes.
const CLICK_CLIP: &str = "sfx/click/click_1.ogg";

/// Time constant (s) of the exponential approach toward the mapped speed — the sim's crank speed
/// steps at the fixed rate, and an unsmoothed `set_speed` per frame reads as a warble.
const ENGINE_SPEED_TAU: f32 = 0.12;

/// Playback-speed band, as fractions of the authored crank band: a lugging engine is held at the
/// speed it would run at [`ENGINE_LUG_FRACTION`] of idle, and an over-revved crank at
/// [`ENGINE_OVERSHOOT_FRACTION`] of the governor. The only two numbers the band costs — the speeds
/// themselves are the vehicle's.
const ENGINE_LUG_FRACTION: f32 = 0.8;
const ENGINE_OVERSHOOT_FRACTION: f32 = 1.05;

pub fn plugin(app: &mut App) {
    app.init_resource::<ViewRng>()
        .init_resource::<LastReportVariant>()
        // The one falloff law for every spatial emitter in the game.
        .insert_resource(DefaultSpatialScale(SpatialScale::new(SPATIAL_SCALE)))
        .add_systems(Startup, setup_sfx_clips)
        .add_observer(on_weapon_report)
        .add_systems(
            Update,
            // Gated exactly as `command::gather_commands` is: a click with the cursor released is
            // menu input, not a trigger pull.
            play_fire_click.in_set(PlayerInputSet).in_set(GameplaySet),
        )
        // Warm before attach so a tank's clips are already in flight when its emitter is born, and
        // attach before drive so a tank that appears this frame is already carrying one.
        .add_systems(
            Update,
            (warm_tank_clips, attach_engine_loops, drive_engine_speed).chain(),
        );
    // Dev-only guard: confirm every clip path resolves, so a renamed or missing file surfaces as a
    // loud error instead of silence.
    #[cfg(debug_assertions)]
    app.add_systems(Update, verify_sfx_clips);
}

/// Every clip this layer can play, keyed by asset path. The click is loaded at Startup from its
/// fixed path; every other entry arrives from a tank's spec-derived components. `AssetServer::load`
/// already dedupes by path — this map is what lets a play site reach a handle without one.
#[derive(Resource)]
struct SfxClips {
    click: Handle<AudioSource>,
    tank: HashMap<String, Handle<AudioSource>>,
}

impl SfxClips {
    /// The handle for `path`, loading it on first sight.
    fn tank_clip(&mut self, asset_server: &AssetServer, path: &str) -> Handle<AudioSource> {
        self.tank
            .entry(path.to_string())
            .or_insert_with(|| asset_server.load(path.to_string()))
            .clone()
    }
}

fn setup_sfx_clips(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(SfxClips {
        click: asset_server.load(CLICK_CLIP),
        tank: HashMap::new(),
    });
}

/// Start every clip a newly built tank declares, so the handles are warm before the first shot or
/// the first idle frame rather than at the moment they are wanted.
fn warm_tank_clips(
    asset_server: Res<AssetServer>,
    mut clips: ResMut<SfxClips>,
    weapons: Query<&Weapon, Added<Weapon>>,
    engines: Query<&EngineSound, Added<EngineSound>>,
) {
    for weapon in &weapons {
        for path in &weapon.report_clips {
            clips.tank_clip(&asset_server, path);
        }
    }
    for engine in &engines {
        clips.tank_clip(&asset_server, &engine.clip);
    }
}

/// Dev-time asset-load guard: each frame, check the load state of every clip the running game has
/// asked for — the click plus whatever the loaded tanks authored — and `error!` any that FAILED (a
/// bad path, a missing file, a codec bevy was not built with — only Vorbis decodes here). Cheap: it
/// reads load states off handles that already exist. `debug_assertions` only, so shipped clients
/// never pay for it.
#[cfg(debug_assertions)]
fn verify_sfx_clips(
    asset_server: Res<AssetServer>,
    clips: Res<SfxClips>,
    mut reported: Local<std::collections::HashSet<String>>,
) {
    use bevy::asset::LoadState;

    let entries = std::iter::once((CLICK_CLIP, &clips.click)).chain(
        clips
            .tank
            .iter()
            .map(|(path, handle)| (path.as_str(), handle)),
    );
    for (path, handle) in entries {
        // One report per path: the set of clips grows as tanks spawn, so this system can never
        // idle off the way a fixed list would.
        if let LoadState::Failed(err) = asset_server.load_state(handle)
            && reported.insert(path.to_string())
        {
            error!("sfx asset failed to load: {path}: {err}");
        }
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
    clips: Res<SfxClips>,
    mut commands: Commands,
) {
    if !bindings.fire_primary.just_pressed(&keys, &mouse) {
        return;
    }
    commands.spawn((
        AudioPlayer::new(clips.click.clone()),
        PlaybackSettings::DESPAWN.with_volume(Volume::Linear(CLICK_VOLUME)),
    ));
}

/// The variant played by the last report, one deep and shared across every reporting weapon in
/// earshot — [`pick_variant`] rolls against it so no take is ever heard twice running.
#[derive(Resource, Default)]
struct LastReportVariant(Option<usize>);

/// Each weapon's report, on the same `FireShell` seam the flash rides. The firing weapon is
/// resolved from the event's own `shooter` (tank root + weapon slot) — the same lookup
/// `net::fire_presentation` uses — and its authored clip list is the whole gate: a weapon with an
/// empty list is silent, and a shell with no shooter (a sandbox or test round) belongs to no
/// weapon at all.
fn on_weapon_report(
    fire: On<FireShell>,
    weapons: Query<(&Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
    asset_server: Res<AssetServer>,
    mut clips: ResMut<SfxClips>,
    mut rng: ResMut<ViewRng>,
    mut last: ResMut<LastReportVariant>,
    mut commands: Commands,
) {
    // Stale remote shot: the report's moment is long past — skip, don't play late.
    if fire.catch_up_ticks > STALE_FIRE_TICKS {
        return;
    }
    let Some(shooter) = fire.shooter else {
        return;
    };
    let Some(weapon) = weapons.iter().find_map(|(weapon, slot, tank)| {
        (slot.0 == shooter.weapon && tank.0 == shooter.tank).then_some(weapon)
    }) else {
        return;
    };
    let Some(variant) = pick_variant(&mut rng, weapon.report_clips.len(), last.0) else {
        return;
    };
    last.0 = Some(variant);
    let clip = clips.tank_clip(&asset_server, &weapon.report_clips[variant]);
    commands.spawn((
        AudioPlayer::new(clip),
        PlaybackSettings::DESPAWN
            .with_spatial(true)
            .with_volume(Volume::Linear(REPORT_VOLUME)),
        Transform::from_translation(fire.origin),
    ));
}

/// One index into a `count`-long clip list, uniform over every take EXCEPT `last`: roll in
/// `0..count-1` and shift the result past `last`, which costs one roll and cannot repeat. `None`
/// for an empty list (a silent weapon); index 0 for a single-take list, which has no other choice
/// to make. A `last` that no longer indexes this list (a different weapon's roll) constrains
/// nothing.
fn pick_variant(rng: &mut ViewRng, count: usize, last: Option<usize>) -> Option<usize> {
    match count {
        0 => None,
        1 => Some(0),
        _ => Some(match last.filter(|&index| index < count) {
            Some(last) => {
                let roll = rng.range(0.0, (count - 1) as f32).floor() as usize;
                roll + usize::from(roll >= last)
            }
            None => rng.range(0.0, count as f32).floor() as usize,
        }),
    }
}

/// Marks a tank whose engine emitter has been attached.
#[derive(Component)]
struct EngineSfx;

/// The looping engine emitter itself — a child of the tank root, so it rides the presented pose and
/// dies with the body. It carries the law its tank's authored engine data resolves to, derived once
/// at attach.
#[derive(Component)]
struct EngineEmitter(EngineSpeedLaw);

/// Give every tank carrying an authored engine recording a looping emitter. `TankTransmission` is
/// replicated to remotes too, so opponents idle and rev on the same code path; a tank whose spec
/// authors no `engine.sound` carries no [`EngineSound`] and stays silent.
fn attach_engine_loops(
    asset_server: Res<AssetServer>,
    mut clips: ResMut<SfxClips>,
    tanks: Query<(Entity, &EngineSound, &TankTransmission), Without<EngineSfx>>,
    mut commands: Commands,
) {
    for (tank, sound, transmission) in &tanks {
        let law = EngineSpeedLaw::for_engine(sound);
        let clip = clips.tank_clip(&asset_server, &sound.clip);
        commands.entity(tank).insert(EngineSfx);
        commands.spawn((
            EngineEmitter(law),
            AudioPlayer::new(clip),
            PlaybackSettings::LOOP
                .with_spatial(true)
                .with_volume(Volume::Linear(ENGINE_VOLUME))
                // Born at the crank's current speed, so a tank spawned under load does not open
                // with a frame of idle.
                .with_speed(law.speed(transmission.0.omega_e / RPM_TO_RAD)),
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
    emitters: Query<(&ChildOf, &EngineEmitter, &SpatialAudioSink)>,
) {
    // Frame-rate-independent exponential approach over ENGINE_SPEED_TAU.
    let blend = 1.0 - (-time.delta_secs() / ENGINE_SPEED_TAU).exp();
    for (parent, emitter, sink) in &emitters {
        let Ok(transmission) = tanks.get(parent.parent()) else {
            continue;
        };
        let target = emitter.0.speed(transmission.0.omega_e / RPM_TO_RAD);
        let current = sink.speed();
        sink.set_speed(current + (target - current) * blend);
    }
}

/// One engine's rpm → playback-speed mapping, resolved from its authored recording and crank band.
///
/// Three laws, no tuned numbers:
///   * FOUR-STROKE FIRING RATE — a cylinder fires once every two revolutions, so the crank pops at
///     `rpm × cylinders / 2 / 60` Hz. At the idle rpm that is the rate the recording must be played
///     at, hence `anchor = pop_hz(idle) / clip_pop_hz`.
///   * ONE OCTAVE OVER THE BAND — playing at the honest rate across the whole band would tape-shift
///     the sample by the full rpm ratio; instead the ratio is raised to
///     `ln 2 / ln(governed/idle)`, which by construction lands the governed crank exactly one
///     octave above the idle anchor.
///   * THE BAND'S EDGES — the speed is clamped to what the law itself yields at
///     [`ENGINE_LUG_FRACTION`] of idle and [`ENGINE_OVERSHOOT_FRACTION`] of governed, so a stalled
///     crank cannot slide into sub-bass and an over-revved one cannot run away.
#[derive(Clone, Copy)]
struct EngineSpeedLaw {
    idle_rpm: f32,
    anchor: f32,
    exponent: f32,
    min: f32,
    max: f32,
}

impl EngineSpeedLaw {
    fn for_engine(sound: &EngineSound) -> Self {
        let pop_hz = |rpm: f32| rpm * sound.cylinders as f32 / 2.0 / 60.0;
        let law = Self {
            idle_rpm: sound.idle_rpm,
            anchor: pop_hz(sound.idle_rpm) / sound.clip_pop_hz,
            exponent: core::f32::consts::LN_2 / (sound.governed_rpm / sound.idle_rpm).ln(),
            // Unbounded while the edges are being measured against the law itself.
            min: f32::NEG_INFINITY,
            max: f32::INFINITY,
        };
        Self {
            min: law.speed(ENGINE_LUG_FRACTION * sound.idle_rpm),
            max: law.speed(ENGINE_OVERSHOOT_FRACTION * sound.governed_rpm),
            ..law
        }
    }

    /// Playback speed for this engine's loop at `rpm`.
    fn speed(&self, rpm: f32) -> f32 {
        let ratio = (rpm / self.idle_rpm).max(0.0);
        (self.anchor * ratio.powf(self.exponent)).clamp(self.min, self.max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic engine, so every expectation below is arithmetic on THESE numbers rather than a
    /// constant copied out of a shipped sheet: 8 cylinders over a 500–2000 rpm band, on a recording
    /// that pops 30 times a second.
    fn fixture() -> EngineSound {
        EngineSound {
            clip: "sfx/engine/fixture.ogg".to_string(),
            clip_pop_hz: 30.0,
            cylinders: 8,
            idle_rpm: 500.0,
            governed_rpm: 2000.0,
        }
    }

    #[test]
    fn idle_lands_the_recording_on_the_four_stroke_firing_rate() {
        let sound = fixture();
        // 500 rpm × 8 cylinders / 2 / 60 = 33.333 Hz of firing over a 30 Hz recording.
        let expected = sound.idle_rpm * sound.cylinders as f32 / 2.0 / 60.0 / sound.clip_pop_hz;
        let law = EngineSpeedLaw::for_engine(&sound);
        assert!((law.speed(sound.idle_rpm) - expected).abs() < 1e-5);
    }

    #[test]
    fn the_governed_crank_lands_one_octave_above_idle() {
        let sound = fixture();
        let law = EngineSpeedLaw::for_engine(&sound);
        let idle = law.speed(sound.idle_rpm);
        let governed = law.speed(sound.governed_rpm);
        assert!(
            (governed - 2.0 * idle).abs() < 1e-5,
            "governed {governed} is not an octave over idle {idle}"
        );
    }

    #[test]
    fn the_band_clamps_at_the_lug_and_overshoot_fractions() {
        let sound = fixture();
        let law = EngineSpeedLaw::for_engine(&sound);
        let floor = law.speed(ENGINE_LUG_FRACTION * sound.idle_rpm);
        let ceiling = law.speed(ENGINE_OVERSHOOT_FRACTION * sound.governed_rpm);
        // A stalled, reversed or runaway crank presents as the band's own edge.
        assert_eq!(law.speed(0.0), floor);
        assert_eq!(law.speed(-100.0), floor);
        assert_eq!(law.speed(0.5 * sound.idle_rpm), floor);
        assert_eq!(law.speed(100.0 * sound.governed_rpm), ceiling);
        // The edges bracket the authored band rather than cutting into it.
        assert!(floor < law.speed(sound.idle_rpm));
        assert!(ceiling > law.speed(sound.governed_rpm));
    }

    #[test]
    fn speed_rises_with_rpm_across_the_band() {
        let sound = fixture();
        let law = EngineSpeedLaw::for_engine(&sound);
        let steps = 40;
        let mut previous = law.speed(sound.idle_rpm);
        for step in 1..=steps {
            let rpm =
                sound.idle_rpm + (sound.governed_rpm - sound.idle_rpm) * step as f32 / steps as f32;
            let speed = law.speed(rpm);
            assert!(speed > previous, "speed fell at {rpm} rpm: {speed}");
            previous = speed;
        }
    }

    /// The shipped sheet parses and deliberately authors NO engine sound (the loop was pulled
    /// until a recording worthy of the HL230 exists). This trips when a new loop is authored —
    /// update it to run the law against the new data.
    #[test]
    fn the_shipped_tank_is_deliberately_engine_silent() {
        let spec: crate::spec::TankSpec =
            ron::de::from_str(include_str!("../../assets/tiger_1/tiger_1.tank.ron"))
                .expect("the shipped sheet parses");
        let engine = spec
            .track
            .powertrain
            .transmission
            .as_ref()
            .and_then(|transmission| transmission.engine.as_ref())
            .expect("the Tiger declares an engine");
        assert!(
            engine.sound.is_none(),
            "engine sound returned without updating this test"
        );
    }

    #[test]
    fn a_variant_is_never_rolled_twice_running() {
        let mut rng = ViewRng::seeded(0x5FD3_2A17);
        let count = 5;
        let mut last = None;
        for _ in 0..2_000 {
            let variant = pick_variant(&mut rng, count, last).expect("a stocked list always picks");
            assert!(variant < count, "rolled {variant} outside 0..{count}");
            assert_ne!(Some(variant), last, "the same take landed twice running");
            last = Some(variant);
        }
    }

    #[test]
    fn every_other_variant_stays_reachable() {
        let mut rng = ViewRng::seeded(0x1234_5678);
        let count = 5;
        // With one take excluded, the remaining four must all still come up.
        let mut seen = [false; 5];
        for _ in 0..2_000 {
            seen[pick_variant(&mut rng, count, Some(2)).expect("a stocked list always picks")] =
                true;
        }
        assert_eq!(seen, [true, true, false, true, true]);
    }

    #[test]
    fn degenerate_lists_are_silence_and_a_single_take() {
        let mut rng = ViewRng::seeded(7);
        assert_eq!(pick_variant(&mut rng, 0, None), None);
        assert_eq!(pick_variant(&mut rng, 1, None), Some(0));
        assert_eq!(pick_variant(&mut rng, 1, Some(0)), Some(0));
        // A stale index from another weapon's longer list constrains nothing.
        assert!(pick_variant(&mut rng, 3, Some(9)).is_some_and(|variant| variant < 3));
    }
}
