//! The game's damage-model view tests, game state only.

use super::super::tests::{
    STEP_S as FRAME_S, census, corner_app_with, default_image_kept, receive, stage_of, step_by,
};
use bevy::camera::visibility::RenderLayers;

use super::*;
use crate::command::{AxisKeys, CrewSwap};
use crate::damage::{CrewStation, KnockoutReason};
use crate::damage_cam::PenetrationReport;
use crate::damage_cam::delivery::tests::{SHOOTER, STRUCK, recorded, report, vocabulary};
use crate::damage_cam::pen_cam::{Exhibits, Surface};
use crate::damage_cam::wire::{BuiltExplanation, ShotExplanation, ShotVocabulary};

/// The game's damage-model knockout record: fixture data.
const RECORD: &str = "s1_through_crew";

/// Fixture frames resolve every authored wait, beat and drive tap window with at least eight samples.
fn frame_s() -> f32 {
    let beats = Beats {
        impact: 0.0,
        shooter: true,
        lesson_s: demo::LESSON_S,
    };
    FRAME_S
        .min(crate::track::drive::drive_feel().tap_window_s.0 as f32 / 8.0)
        .min(RECORD_WAIT_S / 8.0)
        .min(HIT_S / 8.0)
        .min(demo::LESSON_S / 8.0)
        .min((beats.death() - beats.killer()) / 8.0)
}

fn step(app: &mut App) {
    step_by(app, frame_s());
}

/// The game's damage-model timing owner reads the plan actually presented by this fixture.
fn lesson_s(app: &App) -> f32 {
    let exhibit = app
        .world()
        .resource::<Exhibits>()
        .0
        .first()
        .expect("a run presents its record");
    lesson_length(Some(&exhibit.plan))
}

/// Two fixture frames allow receipt and presentation after the authored record wait.
fn deadline(app: &App, beat: Beat) -> f32 {
    let beats = Beats {
        impact: 0.0,
        shooter: true,
        lesson_s: lesson_s(app),
    };
    RECORD_WAIT_S
        + match beat {
            Beat::Hit => 0.0,
            Beat::Lesson => beats.lesson(),
            Beat::Killer => beats.killer(),
            Beat::Death => beats.death(),
        }
        + 2.0 * frame_s()
}

fn reach(app: &mut App, target: Beat) {
    let seconds = deadline(app, target);
    assert!(
        until(app, seconds, |app| match target {
            Beat::Death => victim(app).run().is_none(),
            _ => beat(app) == Some(target),
        }),
        "the run reaches {target:?}"
    );
}

/// Where the fixture's shooter fired from, in the record's frame (m): fixture data, so the killer
/// beat plays.
const MUZZLE_M: [f32; 3] = [400.0, 1.8, 300.0];

/// The player's tank in [`corner_app`](super::super::tests::corner_app) is the [`SHOOTER`] combatant: here it is the one struck.
fn player_life(life: u32) -> TankLife {
    TankLife {
        combatant: SHOOTER,
        life: CombatantLife(life),
    }
}

/// [`RECORD`] fired by `shooter` at `struck`, naming a knockout where `killed`: the struck
/// player's projection, the shooter's, the vocabulary and the report.
fn shot(
    shooter: CombatantId,
    struck: TankLife,
    killed: bool,
) -> (
    ShotExplanation,
    ShotExplanation,
    ShotVocabulary,
    PenetrationReport,
) {
    let mut report = report(RECORD);
    report.killed = killed.then_some(KnockoutReason::CrewLoss);
    report.muzzle = Some(MUZZLE_M);
    let vocabulary = vocabulary(&report);
    let mut recorded = recorded(&report, shooter, 1200, struck);
    recorded.contact_tick = 1201;
    recorded.completion_tick = 1203;
    recorded.outcome = Some(0);
    let built = BuiltExplanation::build(&vocabulary, &recorded).unwrap();
    let mut projections = built.projections().into_iter().map(|(_, e)| e);
    let victim = projections.next().unwrap();
    let shooter = projections.next().unwrap();
    (victim, shooter, vocabulary, report)
}

/// Fixture presentation choices exercise the replay and corner regardless of player defaults.
fn replay_settings() -> Settings {
    Settings {
        kill_cam: KillCamSetting::On,
        pen_cam_corner: crate::settings::PenCamCorner::All,
        reduce_motion: ReduceMotion::Off,
        ..Settings::default()
    }
}

/// The kill cam's app: [`corner_app`](super::super::tests::corner_app) with fixture keys, the
/// Esc router and the player's camera.
fn app(settings: Settings) -> (App, Entity, Entity) {
    app_with(settings, crate::state::escape_router)
}

/// [`app`] with `mount` supplying its input owner before the app finishes.
fn app_with(settings: Settings, mount: impl FnOnce(&mut App)) -> (App, Entity, Entity) {
    let (mut app, player) = corner_app_with(settings, mount);
    // Fixture controls stay independent of the player's default bindings.
    app.insert_resource(Bindings {
        throttle: AxisKeys {
            pos: KeyCode::KeyW,
            neg: KeyCode::KeyS,
        },
        steer: AxisKeys {
            pos: KeyCode::KeyD,
            neg: KeyCode::KeyA,
        },
        fire_primary: ButtonBinding::Mouse(MouseButton::Left),
        fire_secondary: ButtonBinding::Key(KeyCode::Space),
        xray: ButtonBinding::Key(KeyCode::KeyX),
        kill_cam_hold: ButtonBinding::Key(KeyCode::KeyC),
        kill_cam_replay: ButtonBinding::Key(KeyCode::Space),
        ..default()
    })
    .init_resource::<ButtonInput<KeyCode>>()
    .init_resource::<ButtonInput<MouseButton>>();
    let camera = app
        .world_mut()
        .spawn((PlayerView, CameraProfile::BattlefieldThirdPerson))
        .id();
    app.update();
    (app, player, camera)
}

fn knock_out(app: &mut App, player: Entity) {
    app.world_mut().entity_mut(player).insert(TankKnockedOut {
        reason: KnockoutReason::CrewLoss,
    });
}

fn victim(app: &App) -> &Victim {
    app.world().resource::<Victim>()
}

fn gate(app: &App) -> KillCamGate {
    *app.world().resource::<KillCamGate>()
}

fn beat(app: &App) -> Option<Beat> {
    victim(app).run().map(|run| run.beat())
}

fn captive(app: &App) -> bool {
    app.world().resource::<CameraCaptive>().0
}

/// Press `key` for one frame.
fn press(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
    step(app);
    let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    keys.release(key);
    keys.clear();
}

/// Run frames until `done` holds or `seconds` pass; whether it held.
fn until(app: &mut App, seconds: f32, done: impl Fn(&App) -> bool) -> bool {
    if done(app) {
        return true;
    }
    for _ in 0..(seconds / frame_s()).ceil() as usize {
        step(app);
        assert!(
            app.world().resource::<Exhibits>().0.len() <= 1,
            "never two surfaces"
        );
        if let Some(showing) = app.world().resource::<Corner>().showing {
            assert_eq!(showing.surface, Surface::World, "the slot stays closed");
        }
        if done(app) {
            return true;
        }
    }
    false
}

/// Seconds of frames.
fn run(app: &mut App, seconds: f32) {
    until(app, seconds, |_| false);
}

/// A knockout whose killing record arrives `record_after_s` after the fact (negative: before it),
/// the struck tank's stage view stood in once the slot has closed. The app, the player, the camera.
fn killed(record_after_s: f32) -> (App, Entity, Entity) {
    killed_in(app(replay_settings()), record_after_s)
}

/// [`killed`], in `app`.
fn killed_in(
    (mut app, player, camera): (App, Entity, Entity),
    record_after_s: f32,
) -> (App, Entity, Entity) {
    let (explanation, _, vocabulary, report) = shot(STRUCK, player_life(0), true);
    if record_after_s < 0.0 {
        receive(&mut app, explanation.clone(), &vocabulary);
    }
    knock_out(&mut app, player);
    step(&mut app);
    stage_of(&mut app, report.vehicle.as_deref().unwrap());
    if record_after_s >= 0.0 {
        run(&mut app, record_after_s);
        receive(&mut app, explanation, &vocabulary);
    }
    (app, player, camera)
}

/// The game's damage-model card uses the live shooter's loaded data, even for a handle with
/// no disk path. A reparse mutant would lose the in-memory display name this fixture authors.
#[test]
fn the_killer_card_reads_the_live_shooters_loaded_spec() {
    let (mut app, player, camera) = app(replay_settings());
    let record = report(RECORD);
    let path = crate::assets::asset_root().join(record.vehicle.as_deref().unwrap());
    let mut spec: TankSpec = crate::provenance::from_shipped_str(
        crate::tank::TANK_SPEC_PATH,
        &std::fs::read_to_string(path).unwrap(),
    )
    .unwrap();
    spec.display_name = "Loaded fixture vehicle".to_owned();
    let handle = app.world_mut().resource_mut::<Assets<TankSpec>>().add(spec);
    assert!(
        handle.path().is_none(),
        "the fixture has no file to reparse"
    );
    app.world_mut()
        .spawn((STRUCK, Tank, TankSpecHandle(handle)));
    let (app, _, _) = killed_in((app, player, camera), -1.0);
    let killing = victim(&app)
        .record
        .as_ref()
        .expect("the killing record arrived");
    assert_eq!(
        killing.killer.vehicle.as_deref(),
        Some("Loaded fixture vehicle")
    );
}

/// A knockout plays its killing record whichever arrives first, within the window: the hit beat
/// for at least the input dead-zone, the lesson on the world surface, the killer beat on the
/// captive camera, then the death screen with the replay held.
#[test]
fn either_arrival_order_plays_the_four_beats() {
    // Fixture arrivals leave room for receipt and presentation frames inside the authored window.
    let timely_s = RECORD_WAIT_S - 3.0 * frame_s();
    for record_after_s in [-frame_s(), 0.0, timely_s / 2.0, timely_s] {
        let (mut app, _, _) = killed(record_after_s);
        assert!(until(&mut app, 2.0 * frame_s(), |app| victim(app)
            .run()
            .is_some()));
        let zero = victim(&app).death.as_ref().unwrap().zero;
        let since = (app.world().resource::<Time>().elapsed_secs_f64() - zero) as f32;
        // A record later than the hit beat's end opens the lesson at once.
        let first = if since < HIT_S {
            Beat::Hit
        } else {
            Beat::Lesson
        };
        assert_eq!(beat(&app), Some(first), "{record_after_s}");
        assert!(gate(&app).holds_death_screen);
        let lesson = victim(&app).run().unwrap().beats.lesson();
        assert!(lesson >= HIT_S, "{record_after_s}: lesson at {lesson}");
        assert!(lesson + frame_s() >= record_after_s, "{record_after_s}");
        let length = victim(&app).run().unwrap().beats.lesson_s;
        assert_eq!(
            length,
            lesson_s(&app),
            "{record_after_s}: the record's length"
        );

        reach(&mut app, Beat::Lesson);
        let now = app.world().resource::<Time>().elapsed_secs_f64();
        assert!((now - zero) as f32 + frame_s() >= HIT_S);
        step(&mut app);
        assert!(captive(&app));
        let showing = app.world().resource::<Corner>().showing;
        assert!(
            showing.is_some_and(|s| s.surface == Surface::World),
            "{record_after_s}: the lesson draws on the world"
        );
        assert_eq!(app.world().resource::<Exhibits>().0.len(), 1);

        reach(&mut app, Beat::Killer);
        assert!(captive(&app));
        assert!(app.world().resource::<Corner>().showing.is_none());
        assert!(gate(&app).holds_death_screen);

        reach(&mut app, Beat::Death);
        assert!(!captive(&app));
        assert!(!gate(&app).holds_death_screen, "the death screen");
        assert!(victim(&app).record.is_some(), "the replay is held");
    }
}

/// Nothing plays for a knockout whose life has no killing record: none at all, a victim record
/// that names no knockout, a killing record of the player's previous life, or the shooter's copy
/// of the player's own kill. The death screen comes within the window and respawn is live from
/// the dead-zone's end.
#[test]
fn a_knockout_without_its_killing_record_goes_to_the_death_screen_in_the_window() {
    let cases: [(&str, fn(&mut App)); 4] = [
        ("no record", |_| {}),
        ("no knockout named", |app| {
            let (explanation, _, vocabulary, _) = shot(STRUCK, player_life(0), false);
            receive(app, explanation, &vocabulary);
        }),
        ("a previous life", |app| {
            let (explanation, _, vocabulary, _) = shot(STRUCK, player_life(0), true);
            receive(app, explanation, &vocabulary);
            let world = app.world_mut();
            let mut player = world.query_filtered::<&mut CombatantLife, With<Controlled>>();
            *player.single_mut(world).unwrap() = CombatantLife(1);
        }),
        ("the shooter's copy", |app| {
            let struck = TankLife {
                combatant: STRUCK,
                life: CombatantLife(0),
            };
            let (_, shooter, vocabulary, _) = shot(SHOOTER, struck, true);
            receive(app, shooter, &vocabulary);
        }),
    ];
    for (case, setup) in cases {
        let (mut app, player, _) = app(replay_settings());
        setup(&mut app);
        knock_out(&mut app, player);
        step(&mut app);
        assert!(gate(&app).holds_death_screen, "{case}: the window opens");
        assert!(gate(&app).settling, "{case}");
        run(&mut app, HIT_S);
        assert!(!gate(&app).settling, "{case}: respawn is live");
        run(&mut app, (RECORD_WAIT_S - HIT_S).max(0.0) + 2.0 * frame_s());
        assert!(!gate(&app).holds_death_screen, "{case}: the death screen");
        assert!(victim(&app).run().is_none(), "{case}");
        assert!(victim(&app).record.is_none(), "{case}: no replay");
        press(&mut app, KeyCode::Space);
        assert!(victim(&app).run().is_none(), "{case}");
    }
}

/// A killing record that arrives after the window plays nothing on its own; the death screen
/// offers it, and the replay key plays it from the lesson.
#[test]
fn a_late_record_is_offered_for_replay() {
    let (mut app, _, _) = killed(RECORD_WAIT_S.max(HIT_S) + 2.0 * frame_s());
    step(&mut app);
    assert!(victim(&app).run().is_none());
    assert!(!gate(&app).holds_death_screen);
    assert!(victim(&app).record.is_some());
    press(&mut app, KeyCode::Space);
    assert_eq!(beat(&app), Some(Beat::Lesson));
    assert!(gate(&app).holds_death_screen);
    reach(&mut app, Beat::Death);
    assert!(!gate(&app).holds_death_screen);
}

/// Esc ends a run at the death screen; the hold key freezes its clock while down; the replay key
/// plays it again; none of them reads before the dead-zone's end.
#[test]
fn skip_hold_and_replay_respect_the_dead_zone_and_run_clock() {
    let (mut app, _, _) = killed(-1.0);
    press(&mut app, KeyCode::Escape);
    assert!(victim(&app).run().is_some(), "no key in the dead-zone");
    reach(&mut app, Beat::Lesson);

    // Keep the held clock inside the lesson, away from its floating-point boundary.
    let inside_lesson_s = lesson_s(&app) / 8.0;
    run(&mut app, inside_lesson_s);
    let before = victim(&app).run().unwrap();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyC);
    let held_s = deadline(&app, Beat::Death);
    run(&mut app, held_s);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::KeyC);
    let after = victim(&app).run().unwrap();
    assert!(after.real - before.real >= held_s - frame_s());
    assert!(after.t - before.t < frame_s() * 1.5, "the clock held");
    assert_eq!(after.beat(), Beat::Lesson);

    press(&mut app, KeyCode::Escape);
    assert!(victim(&app).run().is_none(), "skipped");
    assert!(!captive(&app));
    assert!(!gate(&app).holds_death_screen);

    press(&mut app, KeyCode::Space);
    assert_eq!(beat(&app), Some(Beat::Lesson), "replayed");
    reach(&mut app, Beat::Death);
    assert!(victim(&app).record.is_some());
}

/// The setting off: the death screen at once, the killing record still held for the card and
/// the replay; switched mid-run, the run ends.
#[test]
fn the_setting_off_goes_straight_to_the_death_screen() {
    let settings = Settings {
        kill_cam: KillCamSetting::Off,
        ..replay_settings()
    };
    let (mut app, player, _) = app(settings);
    let (explanation, _, vocabulary, _) = shot(STRUCK, player_life(0), true);
    receive(&mut app, explanation, &vocabulary);
    knock_out(&mut app, player);
    step(&mut app);
    assert!(!gate(&app).holds_death_screen);
    assert!(victim(&app).run().is_none());
    assert!(victim(&app).record.is_some());

    let (mut app, _, _) = killed(-1.0);
    reach(&mut app, Beat::Lesson);
    app.world_mut().resource_mut::<Settings>().kill_cam = KillCamSetting::Off;
    step(&mut app);
    assert!(victim(&app).run().is_none());
    assert!(!captive(&app));
}

/// A respawn or a disconnect mid-beat leaves nothing of the kill cam and the camera on the
/// third-person profile; a reconnect to the same dead life offers the replay and plays nothing on
/// its own.
#[test]
fn respawn_or_disconnect_mid_beat_leaves_nothing() {
    for (case, at) in [
        ("respawn", Beat::Hit),
        ("respawn", Beat::Lesson),
        ("respawn", Beat::Killer),
        ("disconnect", Beat::Lesson),
        ("disconnect", Beat::Killer),
    ] {
        let (mut app, player, camera) = app(replay_settings());
        let baseline = census(&mut app);
        let (explanation, _, vocabulary, report) = shot(STRUCK, player_life(0), true);
        receive(&mut app, explanation, &vocabulary);
        knock_out(&mut app, player);
        step(&mut app);
        stage_of(&mut app, report.vehicle.as_deref().unwrap());
        reach(&mut app, at);
        if at == Beat::Lesson {
            // The captive placement's profile, which runs in `PostUpdate` with the camera plugin.
            app.world_mut()
                .entity_mut(camera)
                .insert(CameraProfile::KillCamLesson);
        }
        match case {
            "respawn" => {
                app.world_mut().entity_mut(player).insert(TankCommand {
                    respawn: true,
                    ..default()
                });
                step(&mut app);
                assert!(victim(&app).run().is_some(), "{case} at {at:?}: marked");
                step(&mut app);
                assert!(victim(&app).run().is_none(), "{case} at {at:?}");
                let mut player = app.world_mut().entity_mut(player);
                player.remove::<(TankKnockedOut, TankCommand)>();
                player.insert(CombatantLife(1));
            }
            _ => {
                app.world_mut().trigger(CloseCorner);
            }
        }
        run(&mut app, HIT_S + 2.0 * frame_s());
        assert!(victim(&app).run().is_none(), "{case} at {at:?}");
        assert!(!captive(&app), "{case} at {at:?}");
        let world = app.world();
        assert_eq!(
            world.get::<CameraProfile>(camera),
            Some(&CameraProfile::BattlefieldThirdPerson),
            "{case} at {at:?}"
        );
        assert!(
            world.get::<WorldViewer>(camera).is_none(),
            "{case} at {at:?}"
        );
        assert_eq!(census(&mut app), baseline, "{case} at {at:?}: no leaks");
        assert!(default_image_kept(&app), "{case} at {at:?}");
        if case == "disconnect" {
            assert!(!gate(&app).holds_death_screen, "{case}: the death screen");
            assert!(victim(&app).record.is_some(), "{case}: the record is held");
            press(&mut app, KeyCode::Space);
            assert_eq!(beat(&app), Some(Beat::Lesson), "{case}: the replay");
        } else {
            assert_eq!(gate(&app), KillCamGate::default(), "{case}");
            assert!(victim(&app).record.is_none(), "{case}");
        }
    }
}

/// While the kill cam owns the slot, the player's own hit on another tank presented then opens no
/// corner: one surface at a time.
#[test]
fn the_slot_stays_closed_under_the_kill_cam() {
    let (mut app, _, _) = killed(-1.0);
    reach(&mut app, Beat::Lesson);
    let struck = TankLife {
        combatant: STRUCK,
        life: CombatantLife(0),
    };
    let (_, shooter, vocabulary, _) = shot(SHOOTER, struck, false);
    receive(&mut app, shooter, &vocabulary);
    reach(&mut app, Beat::Death);
    assert!(app.world().resource::<Corner>().showing.is_none());
}

/// The kill cam's app with the game's readers of its gate on their own sets: the network client's
/// death screen and respawn key, and the spawn map, under the shipped overlay plugin. The
/// player's tank takes commands.
fn composed() -> (App, Entity, Entity) {
    let (mut app, player, camera) = app_with(replay_settings(), |app| {
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_message::<bevy::window::WindowFocused>();
        crate::flow::in_match_plugin(app);
        crate::overlay::plugin(app);
    });
    // The death screen's countdown reads the fixed timestep.
    app.init_resource::<Time<Fixed>>().insert_resource(UiFonts {
        hud: Handle::default(),
        body: Handle::default(),
    });
    crate::net::mount_death_screen(&mut app);
    crate::net::spawn_map::mount_bare(&mut app);
    app.world_mut()
        .entity_mut(player)
        .insert(TankCommand::default());
    step(&mut app);
    (app, player, camera)
}

/// The death screen, the respawn key and the map read the gate of the frame they run in: from the
/// knockout's own frame no "YOU DIED" while the cam plays, and R or M pressed every frame is taken
/// on the first press at the dead-zone's end and on none before it. The frame it is taken the run
/// and the gate stand; the next, the run, the camera and the gate end together under the death
/// screen.
#[test]
fn the_gate_is_read_on_the_frame_it_is_written() {
    for key in [KeyCode::KeyR, KeyCode::KeyM] {
        let (mut app, player, _) = composed();
        let (explanation, _, vocabulary, report) = shot(STRUCK, player_life(0), true);
        receive(&mut app, explanation, &vocabulary);
        knock_out(&mut app, player);
        let mut staged = false;
        loop {
            press(&mut app, key);
            if !staged {
                stage_of(&mut app, report.vehicle.as_deref().unwrap());
                staged = true;
            }
            let zero = victim(&app)
                .death
                .as_ref()
                .expect("dead from the knockout's frame")
                .zero;
            let now = app.world().resource::<Time>().elapsed_secs_f64();
            let since = (now - zero) as f32;
            let overlays = app.world().resource::<Overlays>();
            let taken = match key {
                KeyCode::KeyR => app.world().get::<TankCommand>(player).unwrap().respawn,
                _ => overlays.contains(Overlay::SpawnMap),
            };
            assert_eq!(taken, since >= HIT_S, "{key:?} at {since} s");
            assert!(victim(&app).run().is_some(), "{key:?} at {since} s");
            assert!(gate(&app).holds_death_screen, "{key:?} at {since} s");
            if taken {
                break;
            }
            assert!(!overlays.contains(Overlay::Death), "{key:?} at {since} s");
            assert!(
                !crate::net::shows_died(app.world_mut()),
                "{key:?} at {since} s"
            );
        }
        step(&mut app);
        assert!(victim(&app).run().is_none(), "{key:?}");
        assert!(!captive(&app), "{key:?}");
        assert!(!gate(&app).holds_death_screen, "{key:?}");
        assert!(
            app.world().resource::<Overlays>().contains(Overlay::Death),
            "{key:?}: the scrim under the card and the replay"
        );
        // "RESPAWNING…" on R; on M, "YOU DIED" under the map.
        assert_eq!(
            crate::net::shows_died(app.world_mut()),
            key == KeyCode::KeyM,
            "{key:?}"
        );
    }
}

/// Esc during a run ends it and opens no menu; the next Esc opens the menu; with no run, Esc opens
/// the menu at once. On the overlay sets, as the network client composes them.
#[test]
fn esc_skips_the_run_before_it_reaches_the_menu() {
    let menu = |app: &App| app.world().resource::<Overlays>().contains(Overlay::Menu);
    let (app, player, camera) = composed();
    let (mut app, _, _) = killed_in((app, player, camera), -1.0);
    reach(&mut app, Beat::Lesson);
    press(&mut app, KeyCode::Escape);
    assert!(victim(&app).run().is_none(), "the run ended");
    assert!(!menu(&app), "and the press opened no menu");
    press(&mut app, KeyCode::Escape);
    assert!(menu(&app), "the next press is the menu's");

    let (mut app, _, _) = composed();
    press(&mut app, KeyCode::Escape);
    assert!(menu(&app), "with no run the press is the menu's");
}

/// The captor's profile draws on the frame its beat begins: the lesson's channels on the lesson's
/// first frame, the third person's on the killer beat's first.
#[test]
fn each_beats_channels_resolve_on_its_first_frame() {
    let (mut app, player, camera) = app(replay_settings());
    crate::render_policy::plugin(&mut app);
    app.init_resource::<avian3d::collider_tree::ColliderTrees>()
        .insert_resource(TurretPivot::at(Vec3::Y));
    app.world_mut().entity_mut(player).insert((
        Tank,
        Transform::IDENTITY,
        crate::tests::tank_views(crate::camera::COMMANDER_FOV_BEFORE_BIND),
    ));
    app.world_mut()
        .entity_mut(camera)
        .insert((Transform::from_xyz(0.0, 3.0, 10.0), Projection::default()));
    let (mut app, _, camera) = killed_in((app, player, camera), -1.0);
    let layers = |app: &App| app.world().get::<RenderLayers>(camera).cloned();
    for (beat_now, profile) in [
        (Beat::Lesson, CameraProfile::KillCamLesson),
        (Beat::Killer, CameraProfile::BattlefieldThirdPerson),
    ] {
        reach(&mut app, beat_now);
        assert_eq!(layers(&app), Some(profile.mask()), "{beat_now:?}");
    }
}

/// The controlled tank's command as the frame's device read leaves it, before the fixed ticks.
#[derive(Resource, Default)]
struct DeviceRead(Option<TankCommand>);

fn read_device(mut read: ResMut<DeviceRead>, own: Query<Option<&TankCommand>, With<Controlled>>) {
    // Before `device` completes the fixture, the controlled tank has no command yet.
    read.0 = own
        .single()
        .expect("the device fixture has one controlled tank")
        .copied();
}

/// The player's device on its real systems beside the kill cam: the gather, the sight toggle, the
/// third-person aim commit, the crew swap and the trigger click, and what the frame's device read
/// leaves ([`DeviceRead`]). Mounted before the app finishes ([`app_with`]); [`device`] gives them
/// their world.
fn mount_device(app: &mut App) {
    crate::state::escape_router(app);
    // Fixture handles: device ownership reads the widgets without shaping or rendering text.
    app.insert_resource(crate::ui_font::UiFonts {
        hud: Handle::default(),
        body: Handle::default(),
    })
    .init_resource::<Time<Fixed>>()
    .init_resource::<crate::WeaponClock>();
    bevy::state::app::StatesPlugin.build(app);
    crate::flow::in_match_plugin(app);
    crate::command::client_plugin(app);
    // The reticle's optic mask material loads its shader.
    app.init_asset::<bevy::shader::Shader>();
    crate::sight::plugin(app);
    crate::aim::client_plugin(app);
    crate::crew_ui::plugin(app);
    crate::sfx::mount_fire_click(app);
    app.init_resource::<DeviceRead>().add_systems(
        RunFixedMainLoop,
        read_device
            .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
            .after(crate::command::TankDeviceSet),
    );
}

/// The world of [`mount_device`]'s systems: their resources, a window and the player's camera to
/// pick through, and a player tank whose views are all open and whose first two crew bar keys are
/// the commander's and the gunner's seats (fixture data).
fn device(app: &mut App, player: Entity, camera: Entity) {
    app.init_resource::<crate::firecontrol::Ranging>()
        .init_resource::<avian3d::collider_tree::ColliderTrees>()
        .init_resource::<bevy::input::mouse::AccumulatedMouseMotion>();
    let world = app.world_mut();
    world.spawn(Window::default());
    let [hull, turret, gun, muzzle] = std::array::from_fn(|_| {
        world
            .spawn((
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
                ChildOf(player),
            ))
            .id()
    });
    world.entity_mut(hull).insert(crate::tank::Hull);
    // FIXTURE DATA: a complete optic has a range table and propagated hull/gun poses at spawn.
    world
        .entity_mut(muzzle)
        .insert(crate::firecontrol::RangeTable::test_rows(&[
            (0.0, 0.0),
            (4000.0, 0.04),
        ]));
    for seat in [CrewStation::Commander, CrewStation::Gunner] {
        world.spawn((
            seat,
            crate::damage::Crewman { home: seat },
            crate::damage::VolumeOf(player),
        ));
    }
    let open = || crate::tank::ViewConfig {
        fov: 0.8,
        requires: Vec::new(),
    };
    world.entity_mut(player).insert((
        crate::tank::Rig {
            hull,
            turret,
            gunner_yaw: gun,
            gun,
            muzzle,
        },
        TankViews {
            gunner: open(),
            commander: open(),
        },
        TankCommand::default(),
        Transform::IDENTITY,
        avian3d::prelude::Position::default(),
        avian3d::prelude::Rotation::default(),
    ));
    world.entity_mut(camera).insert((
        Camera {
            computed: bevy::camera::ComputedCameraValues {
                clip_from_view: Mat4::perspective_infinite_reverse_rh(0.9, 16.0 / 9.0, 0.1),
                target_info: Some(bevy::camera::RenderTargetInfo {
                    physical_size: UVec2::new(1280, 720),
                    scale_factor: 1.0,
                }),
                ..default()
            },
            ..default()
        },
        Transform::from_xyz(0.0, 3.0, 10.0),
    ));
}

/// [`hold_all`]'s frames so far: its crew bar key alternates across calls.
#[derive(Resource, Default)]
struct Held(usize);

/// Every drive, fire and view key down for `frames` frames, the view key pressed anew on each, and
/// the crew bar's `1` and `2` tapped in turn: a swap from the first seat to the second every two
/// frames the swap is read.
fn hold_all(app: &mut App, frames: usize, each: impl Fn(&App)) {
    for _ in 0..frames {
        let mut held = app.world_mut().get_resource_or_insert_with(Held::default);
        let tap = [KeyCode::Digit1, KeyCode::Digit2][held.0 % 2];
        held.0 += 1;
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        for key in [KeyCode::ShiftLeft, KeyCode::Digit1, KeyCode::Digit2] {
            keys.release(key);
        }
        for key in [
            KeyCode::KeyW,
            KeyCode::KeyD,
            KeyCode::Space,
            KeyCode::ShiftLeft,
            tap,
        ] {
            keys.press(key);
        }
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        step(app);
        each(app);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
    }
}

fn let_go(app: &mut App) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .reset_all();
}

/// From the knockout fact to a live tank the death flow owns the device, whichever key replays:
/// drive, fire, aim, the view key and the crew bar held through the run and the death screen leave
/// the command neutral, the view unchanged and the trigger silent, and the replay key replays.
/// Alive before and after, the same keys drive, fire, click, aim, switch the view and swap crew,
/// from the first frame of the new life.
#[test]
fn the_death_flow_owns_the_device() {
    let first_notch = crate::track::drive::drive_feel().notch_step();
    for replay in [
        ButtonBinding::Key(KeyCode::Space),
        ButtonBinding::Mouse(MouseButton::Left),
    ] {
        let (mut app, player, camera) = app_with(replay_settings(), mount_device);
        app.world_mut().resource_mut::<Bindings>().kill_cam_replay = replay;
        device(&mut app, player, camera);
        let command = |app: &App| *app.world().get::<TankCommand>(player).unwrap();
        let mode = |app: &App| *app.world().resource::<SightMode>();
        let clicks = |app: &mut App| {
            app.world_mut()
                .query_filtered::<(), With<AudioPlayer>>()
                .iter(app.world())
                .count()
        };

        hold_all(&mut app, 1, |_| {});
        assert_eq!(clicks(&mut app), 1, "{replay:?}: the trigger clicked");
        let alive = command(&app);
        assert_eq!(
            (alive.throttle, alive.steer),
            (first_notch, first_notch),
            "{replay:?}: the first frame drives in both keys' directions"
        );
        assert!(alive.fire_primary && alive.fire_secondary, "{replay:?}");
        assert!(alive.aim.is_some(), "{replay:?}: aimed");
        assert_eq!(
            mode(&app),
            SightMode::Gunner,
            "{replay:?}: the view switched"
        );
        hold_all(&mut app, 1, |_| {});
        assert_eq!(mode(&app), SightMode::ThirdPerson, "{replay:?}");
        let swap = Some(CrewSwap::Start(CrewStation::Commander, CrewStation::Gunner));
        assert_eq!(command(&app).crew_swap, swap, "{replay:?}: swapped");

        // Down with every control held and the last aim and swap standing from the knockout's frame.
        let (mut app, ..) = killed_in((app, player, camera), -1.0);
        let silent = |app: &App| {
            assert_eq!(command(app), TankCommand::default(), "{replay:?}: neutral");
            assert_eq!(
                app.world().resource::<DeviceRead>().0,
                Some(TankCommand::default()),
                "{replay:?}: nothing read from the device"
            );
            assert_eq!(
                mode(app),
                SightMode::ThirdPerson,
                "{replay:?}: no view change"
            );
        };
        hold_all(&mut app, 8, silent);
        let_go(&mut app);
        reach(&mut app, Beat::Death);
        hold_all(&mut app, 8, silent);
        assert_eq!(beat(&app), Some(Beat::Lesson), "{replay:?}: replayed");
        assert_eq!(
            clicks(&mut app),
            1,
            "{replay:?}: no trigger click while dead"
        );

        let mut tank = app.world_mut().entity_mut(player);
        tank.remove::<TankKnockedOut>();
        tank.insert(CombatantLife(1));
        hold_all(&mut app, 1, |_| {});
        let alive = command(&app);
        assert_eq!(
            (alive.throttle, alive.steer),
            (first_notch, first_notch),
            "{replay:?}: the new life's first frame drives at fresh notches"
        );
        assert!(alive.fire_secondary, "{replay:?}");
        // The first frame's `1` picked the seat the second frame's `2` swaps with.
        hold_all(&mut app, 1, |_| {});
        assert_eq!(command(&app).crew_swap, swap, "{replay:?}: swapped");
    }
}

/// Esc pressed on the very frame a run starts is the run's: the router gave it to the menu on the
/// gate as the frame entered (no run yet), and the run starting takes it back — no menu online, no
/// pause offline, no skip inside the dead-zone.
#[test]
fn esc_on_the_frame_a_run_starts_is_the_runs() {
    let (mut app, player, _) = composed();
    let (explanation, _, vocabulary, _) = shot(STRUCK, player_life(0), true);
    receive(&mut app, explanation, &vocabulary);
    knock_out(&mut app, player);
    press(&mut app, KeyCode::Escape);
    assert!(
        victim(&app).run().is_some(),
        "the run started on that frame"
    );
    assert!(
        !app.world().resource::<Overlays>().contains(Overlay::Menu),
        "and the press opened no menu"
    );
    assert_eq!(
        *app.world().resource::<crate::state::EscapeAction>(),
        crate::state::EscapeAction::None,
        "the press was taken"
    );
}

/// Offline, Esc on the very frame a run starts pauses nothing: the pause toggle orders after the
/// kill cam's drive, which took the press back. The game is Playing on the frame after.
#[test]
fn esc_on_the_frame_a_run_starts_does_not_pause() {
    use crate::state::AppState;

    let (mut app, player, _) = app_with(replay_settings(), |app| {
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_message::<bevy::window::WindowFocused>()
            .init_resource::<Time<avian3d::prelude::Physics>>()
            .insert_resource(UiFonts {
                hud: Handle::default(),
                body: Handle::default(),
            });
        crate::state::sim_plugin(app);
        crate::state::client_plugin(app);
    });
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Playing);
    step(&mut app);
    let (explanation, _, vocabulary, _) = shot(STRUCK, player_life(0), true);
    receive(&mut app, explanation, &vocabulary);
    knock_out(&mut app, player);
    press(&mut app, KeyCode::Escape);
    assert!(
        victim(&app).run().is_some(),
        "the run started on that frame"
    );
    step(&mut app);
    assert_eq!(
        *app.world().resource::<State<AppState>>().get(),
        AppState::Playing,
        "no pause on the run's first frame"
    );
}

/// A player firing as a [`FreeShooter`] at the tank it holds: that tank knocked out by the free
/// shooter's round is no death (no run, no captive camera, no gate held), and the corner shows the
/// round. The armour sandbox's range relies on both.
#[test]
fn a_free_shooters_held_tank_knocked_out_is_no_death() {
    let (mut app, held, _) = app(replay_settings());
    app.world_mut().entity_mut(held).insert(STRUCK);
    app.world_mut()
        .spawn((SHOOTER, CombatantLife(0), FreeShooter));
    step(&mut app);
    let struck = TankLife {
        combatant: STRUCK,
        life: CombatantLife(0),
    };
    let (victim_projection, shooter, vocabulary, report) = shot(SHOOTER, struck, true);
    stage_of(&mut app, report.vehicle.as_deref().unwrap());
    knock_out(&mut app, held);
    receive(&mut app, victim_projection, &vocabulary);
    receive(&mut app, shooter, &vocabulary);
    let mut cornered = false;
    let plan = Plan::new(&report, &crate::damage_cam::shapes::TankShapes::default());
    let corner_s = crate::damage_cam::pen_cam::stage::pace(&plan, plan.kind).duration;
    // Allow receipt and presentation frames around the corner's owned duration.
    for _ in 0..(((timeline::OPEN_LATENCY_S + corner_s) / frame_s()).ceil() as usize + 2) {
        step(&mut app);
        assert!(victim(&app).death.is_none(), "no death");
        assert!(!captive(&app), "the camera stays the player's");
        assert_eq!(gate(&app), KillCamGate::default());
        cornered |= app
            .world()
            .resource::<Corner>()
            .showing
            .is_some_and(|showing| showing.surface == Surface::Corner);
    }
    assert!(cornered, "the corner shows the free shooter's round");
}
