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
use crate::command::{Bindings, TankDeviceSet};
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{EngineSound, Muzzle, TankRoot, Weapon, WeaponIndex};
use crate::track::sim::EngineRpm;
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
        .init_resource::<crate::assets::PresentationLoads>()
        .add_systems(PreStartup, setup_sfx_clips)
        .add_observer(on_weapon_report)
        .add_systems(Update, fire_click())
        // Warm before attach so a tank's clips are already in flight when its emitter is born, and
        // attach before drive so a tank that appears this frame is already carrying one.
        .add_systems(
            Update,
            (warm_tank_clips, attach_engine_loops, drive_engine_speed).chain(),
        );
    // A failed required clip is an asset fault in every build.
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

fn setup_sfx_clips(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut required: ResMut<crate::assets::PresentationLoads>,
) {
    let root = crate::assets::asset_root();
    let mut clips = SfxClips {
        click: asset_server.load(CLICK_CLIP),
        tank: HashMap::new(),
    };
    for (path, field) in required_clips(crate::bake::selected_spec()) {
        verify_clip(
            crate::assets::require_file(&root, &path, &field),
            &path,
            &field,
        );
        let handle = if path == CLICK_CLIP {
            clips.click.clone()
        } else {
            clips.tank_clip(&asset_server, &path)
        };
        required.0.push((path, handle.untyped()));
    }
    commands.insert_resource(clips);
}

/// Bevy's audio loader stores bytes without opening the codec. Construct its exact decoder here
/// so unsupported or corrupt headers refuse at startup, with the owning field, before playback.
fn verify_clip(bytes: Vec<u8>, path: &str, field: &str) {
    assert!(
        path.ends_with(".ogg"),
        "{path}: {field}: author a Vorbis .ogg supported by the shipped audio loader"
    );
    type Decoder = <AudioSource as bevy::audio::Decodable>::Decoder;
    let source = AudioSource {
        bytes: bytes.into(),
    };
    let _decoder = Decoder::new(std::io::Cursor::new(source)).unwrap_or_else(|err| {
        panic!(
            "{path}: {field}: required clip cannot decode: {err} — export a supported Vorbis .ogg"
        )
    });
}

/// Every selected tank clip is checked and warmed before any tank exists. Gun reports belong to
/// the gun registry; the engine loop belongs to the tank sheet. An empty report list and an absent
/// engine sound are authored silence.
fn required_clips(spec: &crate::spec::TankSpec) -> Vec<(String, String)> {
    let mut clips = vec![(CLICK_CLIP.to_owned(), "sfx::CLICK_CLIP".to_owned())];
    let registry = crate::guns::GunRegistry::shipped();
    for weapon in spec.weapons.values() {
        let gun = registry
            .gun(&weapon.gun)
            .expect("the bake certifies every mounted gun");
        for path in gun.report_clips() {
            clips.push((
                path.clone(),
                format!("assets/guns/{}.gun.ron report_clips", weapon.gun),
            ));
        }
    }
    if let Some(sound) = &spec.track.powertrain.transmission.engine.sound {
        clips.push((
            sound.clip.clone(),
            format!(
                "{} track.powertrain.transmission.engine.sound.clip",
                crate::tank::TANK_SPEC_PATH
            ),
        ));
    }
    clips
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

/// Required clips must decode in every build. The set grows as tanks spawn, so successful handles
/// are checked again while a newly authored dependency may still be loading.
fn verify_sfx_clips(asset_server: Res<AssetServer>, clips: Res<SfxClips>) {
    use bevy::asset::LoadState;

    let entries = std::iter::once((CLICK_CLIP, &clips.click)).chain(
        clips
            .tank
            .iter()
            .map(|(path, handle)| (path.as_str(), handle)),
    );
    for (path, handle) in entries {
        if let LoadState::Failed(err) = asset_server.load_state(handle) {
            panic!(
                "{path}: required sfx clip failed to load: {err} — restore a supported Vorbis .ogg or correct its authored path"
            );
        }
    }
}

/// [`play_fire_click`], gated exactly as `command::gather_commands` is: a click with the cursor
/// released is menu input, and one while the death flow owns the device is its key, not a trigger
/// pull.
fn fire_click() -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    play_fire_click
        .in_set(PlayerInputSet)
        .in_set(TankDeviceSet)
        .in_set(GameplaySet)
}

/// [`play_fire_click`] on its production gate, with a stand-in clip.
#[cfg(test)]
pub(crate) fn mount_fire_click(app: &mut App) {
    app.insert_resource(SfxClips {
        click: Handle::default(),
        tank: HashMap::new(),
    })
    .add_systems(Update, fire_click());
}

/// The trigger's own click, straight off the device edge. `TankCommand::fire_primary` is a latched
/// edge a fixed tick consumes, so reading it here would click again on every frame the latch
/// survives — this reads the binding exactly as `command::gather_commands` does. It plays on every
/// physical press: the click is the switch, and the reload gate belongs to the gun.
/// The game's damage model reuses this switch click for the repair button.
fn play_fire_click(
    bindings: Res<Bindings>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    clips: Res<SfxClips>,
    mut commands: Commands,
) {
    if !bindings.fire_primary.just_pressed(&keys, &mouse)
        && !bindings.repair.just_pressed(&keys, &mouse)
    {
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
#[expect(
    clippy::too_many_arguments,
    reason = "the report reads shot identity, source liveness, authored clips and view-only playback state"
)]
fn on_weapon_report(
    fire: On<FireShell>,
    roots: Query<(), With<crate::tank::Tank>>,
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
    let mut matching = weapons.iter().filter_map(|(weapon, slot, tank)| {
        (slot.0 == shooter.weapon && tank.0 == shooter.tank).then_some(weapon)
    });
    let Some(weapon) = matching.next() else {
        assert!(
            !roots.contains(shooter.tank),
            "a live shot source has its complete authored weapon slot"
        );
        return; // A deferred presentation can outlive its despawned source; there is no emitter now.
    };
    assert!(
        matching.next().is_none(),
        "one source and slot identify exactly one weapon"
    );
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

/// Give every tank carrying an authored engine recording a looping emitter. Every tank carries the
/// public [`EngineRpm`] — the own, an opponent, a single-player one — so playback follows the
/// crank on this one code path, including a paused loop at rest; a tank whose spec authors no `engine.sound` carries no [`EngineSound`]
/// and stays silent.
fn attach_engine_loops(
    asset_server: Res<AssetServer>,
    mut clips: ResMut<SfxClips>,
    tanks: Query<(Entity, &EngineSound, &EngineRpm), Without<EngineSfx>>,
    mut commands: Commands,
) {
    for (tank, sound, rpm) in &tanks {
        let law = EngineSpeedLaw::for_engine(sound);
        let clip = clips.tank_clip(&asset_server, &sound.clip);
        commands.entity(tank).insert(EngineSfx);
        let playback = PlaybackSettings {
            paused: rpm.rpm() == 0.0,
            ..PlaybackSettings::LOOP
                .with_spatial(true)
                .with_volume(Volume::Linear(ENGINE_VOLUME))
                .with_speed(law.speed(rpm.rpm()).max(law.min))
        };
        commands.spawn((
            EngineEmitter(law),
            AudioPlayer::new(clip),
            playback,
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
    tanks: Query<&EngineRpm>,
    emitters: Query<(&ChildOf, &EngineEmitter, &SpatialAudioSink)>,
) {
    // Frame-rate-independent exponential approach over ENGINE_SPEED_TAU.
    let blend = 1.0 - (-time.delta_secs() / ENGINE_SPEED_TAU).exp();
    for (parent, emitter, sink) in &emitters {
        let Ok(rpm) = tanks.get(parent.parent()) else {
            continue;
        };
        let target = emitter.0.speed(rpm.rpm());
        if target == 0.0 {
            // The game's damage model: a stopped crank has no running loop.
            sink.pause();
            continue;
        }
        sink.play();
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
///     turning crank cannot slide into sub-bass and an over-revved one cannot run away.
///     A stopped crank returns zero and its emitter pauses.
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
        if rpm <= 0.0 {
            return 0.0;
        }
        let ratio = rpm / self.idle_rpm;
        (self.anchor * ratio.powf(self.exponent)).clamp(self.min, self.max)
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// Every selected tank dependency must exist before admission; another weapon or engine
    /// recording must extend this same list rather than depending on its first play call.
    #[test]
    fn authored_sfx_dependencies_resolve_before_play() {
        let root = crate::assets::asset_root();
        for (path, field) in required_clips(crate::bake::selected_spec()) {
            verify_clip(
                crate::assets::require_file(&root, &path, &field),
                &path,
                &field,
            );
        }
    }

    /// The audio loader accepts raw bytes. Admission must reject a malformed recording before a
    /// weapon's first report reaches the audio thread.
    #[test]
    #[should_panic(expected = "fixture.report_clips: required clip cannot decode")]
    fn a_malformed_clip_is_refused_before_playback() {
        verify_clip(b"not audio".to_vec(), "fixture.ogg", "fixture.report_clips");
    }

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

    /// Idle lands the recording on the four-stroke firing rate, the governed crank one octave
    /// above it, and the speed rises with rpm across the band between them.
    #[test]
    fn the_governed_crank_lands_one_octave_above_idle() {
        let sound = fixture();
        let law = EngineSpeedLaw::for_engine(&sound);
        let idle = law.speed(sound.idle_rpm);
        // 500 rpm × 8 cylinders / 2 / 60 = 33.333 Hz of firing over a 30 Hz recording.
        let firing = sound.idle_rpm * sound.cylinders as f32 / 2.0 / 60.0 / sound.clip_pop_hz;
        assert!((idle - firing).abs() < 1e-5, "idle {idle} against {firing}");
        let governed = law.speed(sound.governed_rpm);
        assert!(
            (governed - 2.0 * idle).abs() < 1e-5,
            "governed {governed} is not an octave over idle {idle}"
        );
        let steps = 40;
        let mut previous = idle;
        for step in 1..=steps {
            let rpm =
                sound.idle_rpm + (sound.governed_rpm - sound.idle_rpm) * step as f32 / steps as f32;
            let speed = law.speed(rpm);
            assert!(speed > previous, "speed fell at {rpm} rpm: {speed}");
            previous = speed;
        }
    }

    /// A replica of an opponent carries the public `EngineRpm` and no `TankTransmission` (owner-only
    /// on the wire): its engine loop must still be born, at the speed the law gives that rpm. A
    /// mutant that reads the crank off `TankTransmission` again leaves every opponent silent.
    #[test]
    fn a_remote_tank_without_a_transmission_rides_its_public_rpm() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<AudioSource>()
            .insert_resource(SfxClips {
                click: Handle::default(),
                tank: HashMap::new(),
            });
        let sound = fixture();
        for rpm in [EngineRpm(0), EngineRpm(1_234)] {
            // Fixture stopped and turning cranks.
            app.world_mut().spawn((sound.clone(), rpm));
        }
        app.world_mut()
            .run_system_once(attach_engine_loops)
            .unwrap();
        let world = app.world_mut();
        let mut emitters = world.query::<(&ChildOf, &PlaybackSettings)>();
        let mut count = 0;
        for (parent, playback) in emitters.iter(world) {
            let rpm = world.get::<EngineRpm>(parent.parent()).unwrap().rpm();
            assert_eq!(
                playback.paused,
                rpm == 0.0,
                "a stopped public crank must be born silent"
            );
            if rpm > 0.0 {
                assert_eq!(
                    playback.speed,
                    EngineSpeedLaw::for_engine(&sound).speed(rpm),
                    "a turning emitter is born at the public crank's speed"
                );
            }
            count += 1;
        }
        assert_eq!(count, 2, "each public crank gets one emitter");
    }

    /// The game's damage model: a crank at rest has no running loop; sound must not hold
    /// its lug pitch after the engine's HP pool empties and its stored energy runs out.
    #[test]
    fn a_stopped_crank_has_no_running_loop() {
        assert_eq!(EngineSpeedLaw::for_engine(&fixture()).speed(0.0), 0.0);
    }

    #[test]
    fn the_band_clamps_at_the_lug_and_overshoot_fractions() {
        let sound = fixture();
        let law = EngineSpeedLaw::for_engine(&sound);
        let floor = law.speed(ENGINE_LUG_FRACTION * sound.idle_rpm);
        let ceiling = law.speed(ENGINE_OVERSHOOT_FRACTION * sound.governed_rpm);
        // The game's damage model: a crank at rest has no running loop; a turning crank
        // stays inside the pitch band.
        assert_eq!(law.speed(0.0), 0.0);
        assert_eq!(law.speed(-100.0), 0.0);
        assert_eq!(law.speed(0.5 * sound.idle_rpm), floor);
        assert_eq!(law.speed(100.0 * sound.governed_rpm), ceiling);
        // The edges bracket the authored band rather than cutting into it.
        assert!(floor < law.speed(sound.idle_rpm));
        assert!(ceiling > law.speed(sound.governed_rpm));
    }

    /// A variant is never rolled twice running, and with one take excluded every other one still
    /// comes up.
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
        let mut rng = ViewRng::seeded(0x1234_5678);
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
