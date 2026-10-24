//! The armour sandbox's range, a tool for the game's damage model: what its gun fires, when its
//! knocked-out target is rebuilt, which shots it draws, its armour overlay, and its own cameras
//! and render layers beside the channels.

use std::time::Duration;

use bevy::asset::AssetPlugin;
use bevy::time::TimeUpdateStrategy;
use bevy::world_serialization::WorldAsset;

use super::*;
use crate::ballistics::{CrossingEnd, VolumeCrossing};
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

/// The rebuild rule alone, on a manual clock, the look not mounted; the asset server loads the
/// presentation it hands the next target.
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

/// A knocked-out target stands until the look has replayed the shot that knocked it out to its
/// hold beat and held it [`REBUILD_BEAT_S`]: timed from the record's first showing when it showed
/// at or after the knockout, from the knockout when the record on show is an earlier shot's (the
/// knockout's not yet shown) or none shows (the look off). The kill cam stands aside for the
/// range's shooter, so the look's replay is where a player reads the knockout; a rebuild inside it
/// cuts it short.
#[test]
fn the_rebuild_waits_out_the_looks_replay_of_the_knockout() {
    // Fixture data: a knockout at 10 s, the knockout's record first shown 0.4 s after it.
    let knocked_out_s = 10.0;
    let shown_s = knocked_out_s + 0.4;
    assert!(
        XRayRelease::held_at(knocked_out_s) > knocked_out_s,
        "the hold beat comes after the first showing"
    );
    let from_knockout = XRayRelease::held_at(knocked_out_s) + REBUILD_BEAT_S;
    assert_eq!(
        rebuild_due(knocked_out_s, None),
        from_knockout,
        "the look off"
    );
    assert_eq!(
        rebuild_due(knocked_out_s, Some(knocked_out_s - 3.0)),
        from_knockout,
        "an earlier shot's record on show"
    );
    assert_eq!(
        rebuild_due(knocked_out_s, Some(shown_s)),
        XRayRelease::held_at(shown_s) + REBUILD_BEAT_S,
        "the knockout's record on show"
    );
}

/// A knocked-out target stands until [`rebuild_due`], then goes and the next life of the target is
/// pending.
#[test]
fn a_knocked_out_target_is_rebuilt_once_due() {
    let mut app = respawn();
    let tank = app.world_mut().spawn((Tank, knocked_out())).id();
    app.update();
    let respawn_s = app.world().resource::<Respawn>();
    let knocked_out_s = respawn_s.knocked_out_s.expect("the knockout is seen");
    assert_eq!(knocked_out_s, now_s(&app));
    let due = respawn_s
        .due_s
        .expect("a knocked-out target is due a rebuild");
    assert_eq!(due, rebuild_due(knocked_out_s, None));
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
    let respawn_s = app.world().resource::<Respawn>();
    assert_eq!((respawn_s.knocked_out_s, respawn_s.due_s), (None, None));
}

/// Clearing the knockout (`C`) cancels the rebuild, and a later knockout waits the whole time again
/// rather than inheriting the cancelled one's.
#[test]
fn clearing_the_knockout_cancels_the_rebuild() {
    let wait = rebuild_due(0.0, None);
    let mut app = respawn();
    let tank = app.world_mut().spawn((Tank, knocked_out())).id();
    app.update();
    app.world_mut().entity_mut(tank).remove::<TankKnockedOut>();
    app.update();
    assert_eq!(app.world().resource::<Respawn>().due_s, None);
    let until = now_s(&app) + 2.0 * wait;
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
    assert!((due - now_s(&app) - wait).abs() < 1e-9);
    assert!(app.world().get_entity(tank).is_ok());
}

/// A shot met a tank when its log bounced off, crossed or ran a line through a volume of a tank,
/// or burst with a field that walks one; a plate's crossing, a burst over no tank and a failed walk
/// did not. The range draws the march of a shot that met no tank and leaves the rest to the look:
/// a miss here draws a tank shot's march over the look, or hides a plate shot's.
#[test]
fn a_shot_met_a_tank_by_its_log() {
    let mut world = World::new();
    let [tank_volume, plate] = [(); 2].map(|()| world.spawn_empty().id());
    let of_tank = |volume: Entity| volume == tank_volume;
    let through = |volume: Entity| VolumeCrossing {
        volume,
        entry: Vec3::ZERO,
        end: Vec3::X,
        chord: 0.1,
        cost: 10.0,
        perforated: true,
    };
    let bounce = |struck: Entity| ArmourMark::Bounce {
        position: Vec3::ZERO,
        normal: Vec3::Z,
        incoming: Vec3::NEG_Z,
        outgoing: Vec3::Y,
        speed_in: 700.0,
        speed_out: 500.0,
        struck,
        shock: 0.0,
    };
    let crossing = |volumes: Vec<VolumeCrossing>| ArmourMark::Crossing {
        entrance: Vec3::ZERO,
        normal: Vec3::Z,
        incoming: Vec3::NEG_Z,
        speed_in: 700.0,
        overmatched: false,
        volumes,
        end: CrossingEnd::Exit {
            at: Vec3::X,
            speed: 600.0,
        },
        cones: 0,
    };
    let burst = |tank: Option<Entity>| ArmourMark::Burst {
        at: Vec3::ZERO,
        speed: 0.0,
        after: 0.0,
        nose: None,
        tank,
    };
    let wound = ArmourMark::Wound {
        incoming: Vec3::NEG_Z,
        speed: 600.0,
        volumes: vec![through(tank_volume)],
    };
    for (log, met) in [
        (vec![bounce(tank_volume)], true),
        (vec![bounce(plate)], false),
        (
            vec![
                crossing(vec![through(plate)]),
                crossing(vec![through(tank_volume)]),
            ],
            true,
        ),
        (vec![crossing(vec![through(plate)])], false),
        (vec![wound], true),
        (vec![burst(Some(tank_volume))], true),
        (vec![burst(None)], false),
        (vec![ArmourMark::Failed], false),
        (Vec::new(), false),
    ] {
        assert_eq!(met_a_tank(&log, of_tank), met, "{log:?}");
    }
}

/// A spent shell that met a tank goes; a spent shell that met only a plate stays on the board with
/// its march. The look draws the shell of a shot on the tank; the range's frozen one would stand
/// beside it.
#[test]
fn a_spent_tank_shot_goes_and_a_plate_shot_stays() {
    let mut app = App::new();
    app.add_systems(Update, sweep_tank_shots);
    let world = app.world_mut();
    let tank = world.spawn_empty().id();
    let tank_volume = world.spawn(VolumeOf(tank)).id();
    let plate = world.spawn_empty().id();
    let mut spent = |struck: Entity| {
        let marks = PenetrationMarks {
            log: vec![ArmourMark::Bounce {
                position: Vec3::ZERO,
                normal: Vec3::Z,
                incoming: Vec3::NEG_Z,
                outgoing: Vec3::Y,
                speed_in: 700.0,
                speed_out: 500.0,
                struck,
                shock: 0.0,
            }],
            ..default()
        };
        world.spawn((ShellPath::default(), marks)).id()
    };
    let tank_shot = spent(tank_volume);
    let plate_shot = spent(plate);
    app.update();
    assert!(app.world().get_entity(tank_shot).is_err(), "the tank shot");
    assert!(app.world().get_entity(plate_shot).is_ok(), "the plate shot");
    assert!(!app.world().entity(plate_shot).contains::<MetTank>());
}

/// The armour overlay draws a root copy of each render primitive of an armour volume's view, at its
/// world pose, on the overlay layer, and nothing of another part's view; a copy shows only once its
/// layer reads the overlay's (`render_policy` puts a new mesh on the battlefield channel, which the
/// main pass draws); every copy goes with the overlay off. A copy under the target would be dressed
/// by the look; a component in the overlay would read as armour.
#[test]
fn the_armour_overlay_copies_the_armour_alone() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<StandardMaterial>()
        .init_resource::<ArmourOverlay>()
        .add_systems(Startup, setup_overlay_materials)
        .add_systems(Update, draw_armour_overlay);
    let pose = Transform::from_xyz(1.0, 2.0, 3.0).with_rotation(Quat::from_rotation_y(0.5));
    let world = app.world_mut();
    let plate = world.spawn(ArmorVolume).id();
    let part = world.spawn_empty().id();
    let mut primitive = |part: Entity, name: &str, at: Transform| {
        let view = world.spawn(ViewOf(part)).id();
        let node = world.spawn(ChildOf(view)).id();
        world
            .spawn((
                Mesh3d::default(),
                GltfMeshName(name.to_owned()),
                GlobalTransform::from(at),
                ChildOf(node),
            ))
            .id()
    };
    let plate_mesh = primitive(plate, "plate", pose);
    primitive(part, "part", Transform::IDENTITY);
    let copies = |app: &mut App| {
        let world = app.world_mut();
        world
            .query::<(
                &ArmourCopy,
                &Transform,
                &RenderLayers,
                &Visibility,
                Has<ChildOf>,
            )>()
            .iter(world)
            .map(|(copy, at, layers, visibility, parented)| {
                (copy.of, *at, layers.clone(), *visibility, parented)
            })
            .collect::<Vec<_>>()
    };
    app.update();
    assert!(copies(&mut app).is_empty(), "off by default");

    *app.world_mut().resource_mut::<ArmourOverlay>() = ArmourOverlay::Opaque;
    app.update();
    let overlay = RenderLayers::layer(OVERLAY_LAYER);
    let found = copies(&mut app);
    assert_eq!(found.len(), 1, "one copy, the plate's");
    let (of, at, layers, visibility, parented) = found[0].clone();
    assert_eq!(of, plate_mesh);
    assert!(!parented, "a root");
    assert!(at.translation.abs_diff_eq(pose.translation, 1e-5));
    assert!(at.rotation.abs_diff_eq(pose.rotation, 1e-5));
    assert_eq!((layers, visibility), (overlay.clone(), Visibility::Hidden));
    app.update();
    assert_eq!(copies(&mut app)[0].3, Visibility::Visible);

    let copy = {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<ArmourCopy>>()
            .single(world)
            .unwrap()
    };
    app.world_mut()
        .entity_mut(copy)
        .insert(RenderLayers::layer(0));
    app.update();
    let found = copies(&mut app);
    assert_eq!(
        (found[0].2.clone(), found[0].3),
        (overlay, Visibility::Hidden),
        "re-pinned, hidden until it reads the overlay's layer"
    );

    *app.world_mut().resource_mut::<ArmourOverlay>() = ArmourOverlay::Off;
    app.update();
    assert!(copies(&mut app).is_empty(), "off");
}

/// The free-fly camera is the range's one player view: the look's masks, the LOD ladders and the
/// HUD read the single `PlayerView`, and the overlay and UI cameras carry none.
#[test]
fn the_free_fly_camera_is_the_one_player_view() {
    let mut app = App::new();
    app.add_systems(Startup, spawn_camera);
    app.update();
    let world = app.world_mut();
    let views: Vec<bool> = world
        .query_filtered::<Has<FreeFlyCam>, With<PlayerView>>()
        .iter(world)
        .collect();
    assert_eq!(views, vec![true]);
    let cameras = world.query::<&Camera>().iter(world).count();
    assert_eq!(cameras, 3, "the main pass, the overlay and the UI");
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
