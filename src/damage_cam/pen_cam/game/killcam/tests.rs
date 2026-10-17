use super::super::tests::{
    STEP_S, census, corner_app, default_image_kept, receive, stage_of, step,
};
use super::*;
use crate::damage::KnockoutReason;
use crate::damage_cam::PenetrationReport;
use crate::damage_cam::delivery::tests::{SHOOTER, STRUCK, recorded, report, vocabulary};
use crate::damage_cam::pen_cam::{Exhibits, Surface};
use crate::damage_cam::wire::{BuiltExplanation, ShotExplanation, ShotVocabulary};

/// The recorded scenario every test kills with: fixture data.
const RECORD: &str = "s1_through_crew";

/// Where the fixture's shooter fired from, in the record's frame (m): fixture data, so the killer
/// beat plays.
const MUZZLE_M: [f32; 3] = [400.0, 1.8, 300.0];

/// The player's tank in [`corner_app`] is the [`SHOOTER`] combatant: here it is the one struck.
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

/// The kill cam's app: [`corner_app`] with the default keys and the player's camera.
fn app(settings: Settings) -> (App, Entity, Entity) {
    let (mut app, player) = corner_app(settings);
    app.insert_resource(Bindings::default())
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
    victim(app).run.map(|run| run.beat())
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
    for _ in 0..(seconds / STEP_S).ceil() as usize {
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
    let (mut app, player, camera) = app(Settings::default());
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

/// A knockout plays its killing record whichever arrives first, within the window: the hit beat
/// for at least the input dead-zone, the lesson on the world surface, the killer beat on the
/// captive camera, then the death screen with the replay held.
#[test]
fn either_arrival_order_plays_the_four_beats() {
    for record_after_s in [-1.0, 0.0, 0.3, 0.8] {
        let (mut app, _, _) = killed(record_after_s);
        assert!(until(&mut app, 0.1, |app| victim(app).run.is_some()));
        // A record later than the hit beat's end opens the lesson at once.
        let first = if record_after_s + 2.0 * STEP_S < HIT_S {
            Beat::Hit
        } else {
            Beat::Lesson
        };
        assert_eq!(beat(&app), Some(first), "{record_after_s}");
        assert!(gate(&app).holds_death_screen);
        let zero = victim(&app).dead.unwrap().1;
        let lesson = victim(&app).run.unwrap().beats.lesson();
        assert!(lesson >= HIT_S, "{record_after_s}: lesson at {lesson}");
        assert!(lesson + 1e-3 >= record_after_s, "{record_after_s}");

        assert!(until(&mut app, 1.5, |app| beat(app) == Some(Beat::Lesson)));
        let now = app.world().resource::<Time>().elapsed_secs_f64();
        assert!((now - zero) as f32 + STEP_S >= HIT_S);
        step(&mut app);
        assert!(captive(&app));
        let showing = app.world().resource::<Corner>().showing;
        assert!(
            showing.is_some_and(|s| s.surface == Surface::World),
            "{record_after_s}: the lesson draws on the world"
        );
        assert_eq!(app.world().resource::<Exhibits>().0.len(), 1);

        assert!(until(&mut app, 5.0, |app| beat(app) == Some(Beat::Killer)));
        assert!(captive(&app));
        assert!(app.world().resource::<Corner>().showing.is_none());
        assert!(gate(&app).holds_death_screen);

        assert!(until(&mut app, 5.0, |app| victim(app).run.is_none()));
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
        let (mut app, player, _) = app(Settings::default());
        setup(&mut app);
        knock_out(&mut app, player);
        step(&mut app);
        assert!(gate(&app).holds_death_screen, "{case}: the window opens");
        assert!(gate(&app).settling, "{case}");
        run(&mut app, HIT_S);
        assert!(!gate(&app).settling, "{case}: respawn is live");
        run(&mut app, RECORD_WAIT_S - HIT_S + 2.0 * STEP_S);
        assert!(!gate(&app).holds_death_screen, "{case}: the death screen");
        assert!(victim(&app).run.is_none(), "{case}");
        assert!(victim(&app).record.is_none(), "{case}: no replay");
        press(&mut app, KeyCode::Space);
        assert!(victim(&app).run.is_none(), "{case}");
    }
}

/// A killing record that arrives after the window plays nothing on its own; the death screen
/// offers it, and the replay key plays it from the lesson.
#[test]
fn a_late_record_is_offered_for_replay() {
    let (mut app, _, _) = killed(RECORD_WAIT_S + 0.5);
    step(&mut app);
    assert!(victim(&app).run.is_none());
    assert!(!gate(&app).holds_death_screen);
    assert!(victim(&app).record.is_some());
    press(&mut app, KeyCode::Space);
    assert_eq!(beat(&app), Some(Beat::Lesson));
    assert!(gate(&app).holds_death_screen);
    assert!(until(&mut app, 10.0, |app| victim(app).run.is_none()));
    assert!(!gate(&app).holds_death_screen);
}

/// The skip key ends a run at the death screen; the hold key freezes its clock while down; the
/// replay key plays it again; none of them reads before the dead-zone's end.
#[test]
fn skip_hold_and_replay() {
    let (mut app, _, _) = killed(-1.0);
    press(&mut app, KeyCode::KeyX);
    assert!(victim(&app).run.is_some(), "no key in the dead-zone");
    assert!(until(&mut app, 1.0, |app| beat(app) == Some(Beat::Lesson)));
    run(&mut app, 0.2);

    let before = victim(&app).run.unwrap();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyC);
    run(&mut app, 1.0);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::KeyC);
    let after = victim(&app).run.unwrap();
    assert!(after.real - before.real >= 1.0 - STEP_S);
    assert!(after.t - before.t < STEP_S * 1.5, "the clock held");
    assert_eq!(after.beat(), Beat::Lesson);

    press(&mut app, KeyCode::KeyX);
    assert!(victim(&app).run.is_none(), "skipped");
    assert!(!captive(&app));
    assert!(!gate(&app).holds_death_screen);

    press(&mut app, KeyCode::Space);
    assert_eq!(beat(&app), Some(Beat::Lesson), "replayed");
    assert!(until(&mut app, 10.0, |app| victim(app).run.is_none()));
    assert!(victim(&app).record.is_some());
}

/// The setting off: the death screen at once, the killing record still held for the card and
/// the replay; switched mid-run, the run ends.
#[test]
fn the_setting_off_goes_straight_to_the_death_screen() {
    let settings = Settings {
        kill_cam: KillCamSetting::Off,
        ..Settings::default()
    };
    let (mut app, player, _) = app(settings);
    let (explanation, _, vocabulary, _) = shot(STRUCK, player_life(0), true);
    receive(&mut app, explanation, &vocabulary);
    knock_out(&mut app, player);
    step(&mut app);
    assert!(!gate(&app).holds_death_screen);
    assert!(victim(&app).run.is_none());
    assert!(victim(&app).record.is_some());

    let (mut app, _, _) = killed(-1.0);
    assert!(until(&mut app, 1.0, |app| beat(app) == Some(Beat::Lesson)));
    app.world_mut().resource_mut::<Settings>().kill_cam = KillCamSetting::Off;
    step(&mut app);
    assert!(victim(&app).run.is_none());
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
        let (mut app, player, camera) = app(Settings::default());
        let baseline = census(&mut app);
        let (explanation, _, vocabulary, report) = shot(STRUCK, player_life(0), true);
        receive(&mut app, explanation, &vocabulary);
        knock_out(&mut app, player);
        step(&mut app);
        stage_of(&mut app, report.vehicle.as_deref().unwrap());
        assert!(until(&mut app, 6.0, |app| beat(app) == Some(at)));
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
                assert!(victim(&app).run.is_none(), "{case} at {at:?}");
                let mut player = app.world_mut().entity_mut(player);
                player.remove::<(TankKnockedOut, TankCommand)>();
                player.insert(CombatantLife(1));
            }
            _ => {
                app.world_mut().trigger(CloseCorner);
            }
        }
        run(&mut app, 0.5);
        assert!(victim(&app).run.is_none(), "{case} at {at:?}");
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
    assert!(until(&mut app, 1.0, |app| beat(app) == Some(Beat::Lesson)));
    let struck = TankLife {
        combatant: STRUCK,
        life: CombatantLife(0),
    };
    let (_, shooter, vocabulary, _) = shot(SHOOTER, struck, false);
    receive(&mut app, shooter, &vocabulary);
    assert!(until(&mut app, 10.0, |app| victim(app).run.is_none()));
    assert!(app.world().resource::<Corner>().showing.is_none());
}
