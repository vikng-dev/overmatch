//! The armour sandbox's range, a tool for the game's damage model: what its gun fires, when its
//! knocked-out target is rebuilt, and its own render layers beside the channels.

use std::time::Duration;

use bevy::asset::AssetPlugin;
use bevy::time::TimeUpdateStrategy;
use bevy::world_serialization::WorldAsset;

use super::*;
use crate::command::ButtonBinding;
use crate::damage::KnockoutReason;

/// The fields of a fired shell the range decides.
#[derive(Debug, PartialEq)]
struct Fired {
    origin: Vec3,
    direction: Dir3,
    speed: f32,
    caliber: f32,
    mass: f32,
    nose_factor: f32,
    penetrator_caliber: f32,
    penetrator_mass: f32,
    wound: f32,
    mechanism: spec::FireMechanism,
    tracer: bool,
    source: Option<(Entity, usize, u8)>,
    shot: Option<ShotId>,
}

impl Fired {
    fn of(fire: &FireShell) -> Self {
        Self {
            origin: fire.origin,
            direction: fire.direction,
            speed: fire.speed,
            caliber: fire.caliber,
            mass: fire.mass,
            nose_factor: fire.nose_factor,
            penetrator_caliber: fire.penetrator_caliber,
            penetrator_mass: fire.penetrator_mass,
            wound: fire.wound,
            mechanism: fire.mechanism,
            tracer: fire.tracer,
            source: fire
                .shooter
                .map(|source| (source.tank, source.weapon, source.round)),
            shot: fire.shot,
        }
    }
}

/// Every shell the range fired, in order.
#[derive(Resource, Default)]
struct Shells(Vec<Fired>);

/// The range's trigger and round selector over a free shooter holding the seed gun, its camera
/// posed off the origin and turned; the shells it fires are collected in [`Shells`].
fn range() -> (App, Entity) {
    let mut app = App::new();
    app.init_resource::<Bindings>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<ShotParams>()
        .init_resource::<ShotClock>()
        .init_resource::<Shells>()
        .add_observer(|fire: On<FireShell>, mut shells: ResMut<Shells>| {
            shells.0.push(Fired::of(&fire));
        })
        .add_systems(Update, (select_round, fire).chain());
    let camera = app
        .world_mut()
        .spawn((
            FreeFlyCam,
            Transform::from_xyz(1.0, 2.0, 3.0).looking_to(Vec3::new(1.0, -0.2, -1.0), Vec3::Y),
            RANGE_SHOOTER,
        ))
        .id();
    app.world_mut().spawn((
        RangeGun,
        seed_gun(),
        WeaponIndex(0),
        TankRoot(camera),
        ChildOf(camera),
    ));
    (app, camera)
}

/// One frame with `binding` freshly pressed and nothing else held.
fn tap(app: &mut App, binding: fn(&Bindings) -> ButtonBinding) {
    let binding = binding(app.world().resource::<Bindings>());
    let world = app.world_mut();
    world.resource_mut::<ButtonInput<KeyCode>>().reset_all();
    world.resource_mut::<ButtonInput<MouseButton>>().reset_all();
    match binding {
        ButtonBinding::Key(key) => world.resource_mut::<ButtonInput<KeyCode>>().press(key),
        ButtonBinding::Mouse(button) => world
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(button),
    }
    app.update();
}

/// The shell the range fires for `round` from `camera` at `speed` on `tick`: the round's data,
/// whole, as the game's mapping makes it.
fn expected(app: &App, camera: Entity, index: u8, speed: f32, tick: u32) -> Fired {
    let gun = seed_gun();
    let round = &gun.rounds[usize::from(index)].round;
    let pose = app.world().get::<Transform>(camera).expect("the camera");
    Fired {
        origin: pose.translation,
        direction: pose.forward(),
        speed,
        caliber: round.body.caliber,
        mass: round.body.mass,
        nose_factor: round.body.nose_factor,
        penetrator_caliber: round.penetrator.caliber,
        penetrator_mass: round.penetrator.mass,
        wound: round.wound,
        mechanism: gun.fire_mode.mechanism(),
        tracer: true,
        source: Some((camera, 0, index)),
        shot: Some(ShotId {
            combatant: RANGE_SHOOTER,
            weapon: 0,
            fire_tick: tick,
        }),
    }
}

/// The range fires the SELECTED round of its gun as that round's data states, from the camera down
/// its view axis, named as the free shooter's round on the clock's tick; the override replaces the
/// muzzle speed and nothing else; a step through the rounds wraps and drops the override. The pen
/// cam's record of a range round reads its round from `ShotSource::round` and its body from the
/// shell, so a shell that disagreed with its named round would be explained as a round it was not.
#[test]
fn the_range_fires_the_selected_round_as_its_data_states() {
    let (mut app, camera) = range();
    let gun = seed_gun();
    let last = u8::try_from(gun.rounds.len() - 1).expect("a u8 round index");
    // Fixture: the seed gun's first and last rounds differ, and the last has a penetrator that is
    // not its body, so the assertions below tell the selected round from the default one and the
    // penetrator from the body.
    assert_ne!(gun.rounds[0], gun.rounds[usize::from(last)]);
    let last_round = gun.rounds[usize::from(last)].round;
    assert_ne!(last_round.penetrator, last_round.body);

    app.world_mut().resource_mut::<ShotParams>().muzzle_speed = Some(500.0);
    tap(&mut app, |bindings| bindings.round_previous);
    assert!(
        *app.world().resource::<ShotParams>()
            == ShotParams {
                round: last,
                muzzle_speed: None,
            },
        "a step back from the first round wraps to the last and drops the override"
    );

    app.world_mut().resource_mut::<ShotClock>().0 = 7;
    tap(&mut app, |bindings| bindings.fire_primary);
    let own = expected(&app, camera, last, last_round.muzzle_speed, 7);
    assert_eq!(app.world().resource::<Shells>().0, vec![own]);

    let override_speed = last_round.muzzle_speed * 0.5;
    app.world_mut().resource_mut::<ShotParams>().muzzle_speed = Some(override_speed);
    app.world_mut().resource_mut::<ShotClock>().0 = 8;
    tap(&mut app, |bindings| bindings.fire_primary);
    let overridden = expected(&app, camera, last, override_speed, 8);
    assert_eq!(app.world().resource::<Shells>().0.last(), Some(&overridden));
}

/// A second trigger on the same shot-clock tick fires nothing: a round's `ShotId` is its tick, and
/// two rounds on one tick would be one shot to the recorder and the delivery, which key on it. The
/// next tick fires again.
#[test]
fn the_range_fires_one_round_per_tick() {
    let (mut app, _) = range();
    app.world_mut().resource_mut::<ShotClock>().0 = 3;
    tap(&mut app, |bindings| bindings.fire_primary);
    tap(&mut app, |bindings| bindings.fire_primary);
    assert_eq!(app.world().resource::<Shells>().0.len(), 1);
    app.world_mut().resource_mut::<ShotClock>().0 = 4;
    tap(&mut app, |bindings| bindings.fire_primary);
    let ticks: Vec<_> = app
        .world()
        .resource::<Shells>()
        .0
        .iter()
        .map(|shell| shell.shot.map(|shot| shot.fire_tick))
        .collect();
    assert_eq!(ticks, vec![Some(3), Some(4)]);
}

/// One frame of virtual time: under the virtual clock's maximum step, so each update advances it
/// by exactly this.
const STEP: Duration = Duration::from_millis(100);

/// The rebuild rule alone, on a manual clock; the asset server loads the presentation it hands
/// the next target.
fn respawn() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<WorldAsset>()
        .init_asset::<spec::TankSpec>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(STEP))
        .init_resource::<TargetLife>()
        .init_resource::<Respawn>()
        .add_systems(Update, respawn_target);
    app
}

fn now_s(app: &App) -> f64 {
    app.world().resource::<Time<Virtual>>().elapsed_secs_f64()
}

fn knocked_out() -> TankKnockedOut {
    TankKnockedOut {
        reason: KnockoutReason::CrewLoss,
    }
}

/// A knocked-out target stands [`RESPAWN_DELAY_S`] of virtual time, then goes and the next life of
/// the target is pending. The kill cam stands aside for the range's shooter, so this delay is the
/// only time a player has to read the knockout on the target.
#[test]
fn a_knocked_out_target_is_rebuilt_after_the_delay() {
    let mut app = respawn();
    let tank = app.world_mut().spawn((Tank, knocked_out())).id();
    app.update();
    let due = app
        .world()
        .resource::<Respawn>()
        .due_s
        .expect("a knocked-out target is due a rebuild");
    assert!((due - now_s(&app) - RESPAWN_DELAY_S).abs() < 1e-9);
    loop {
        app.update();
        if now_s(&app) >= due {
            break;
        }
        assert!(app.world().get_entity(tank).is_ok(), "rebuilt early");
        assert!(!app.world().contains_resource::<PendingTarget>());
    }
    assert!(app.world().get_entity(tank).is_err(), "the target stands");
    assert!(app.world().contains_resource::<PendingTarget>());
    assert_eq!(app.world().resource::<TargetLife>().0, 1);
    assert_eq!(app.world().resource::<Respawn>().due_s, None);
}

/// Clearing the knockout (`C`) cancels the rebuild, and a later knockout waits the whole delay
/// again rather than inheriting the cancelled one's time.
#[test]
fn clearing_the_knockout_cancels_the_rebuild() {
    let mut app = respawn();
    let tank = app.world_mut().spawn((Tank, knocked_out())).id();
    app.update();
    app.world_mut().entity_mut(tank).remove::<TankKnockedOut>();
    app.update();
    assert_eq!(app.world().resource::<Respawn>().due_s, None);
    let until = now_s(&app) + 2.0 * RESPAWN_DELAY_S;
    while now_s(&app) < until {
        app.update();
    }
    assert!(
        app.world().get_entity(tank).is_ok(),
        "a cleared target is rebuilt"
    );
    assert_eq!(app.world().resource::<TargetLife>().0, 0);

    app.world_mut().entity_mut(tank).insert(knocked_out());
    app.update();
    let due = app
        .world()
        .resource::<Respawn>()
        .due_s
        .expect("a fresh knockout is due a rebuild");
    assert!((due - now_s(&app) - RESPAWN_DELAY_S).abs() < 1e-9);
    assert!(app.world().get_entity(tank).is_ok());
}

/// The overlay and HUD cameras' layers are no render channel. This root mounts the channel
/// resolver for the pen cam's cameras, and a camera that drew a channel's layer would draw that
/// channel's geometry: the corner's stage or masks over the battlefield, or the battlefield into
/// the HUD.
#[test]
fn the_sandbox_layers_are_no_render_channel() {
    for layer in [OVERLAY_LAYER, UI_LAYER] {
        assert!(
            !crate::render_policy::is_channel_layer(layer),
            "layer {layer} is a render channel's"
        );
    }
}
