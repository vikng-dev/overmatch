//! The pen cam recorder in the game's fixed-tick sim ([`crate::damage_cam::recorder`]): rounds fired
//! through [`FireShell`] at generic fixture tanks, the real march, the damage consequences and the
//! physics step, read back as [`RecordedShot`]s. Every geometry, HP pool and speed here is fixture
//! data; the assertions are class invariants.

use std::time::Duration;

use avian3d::prelude::{CollisionLayers, LayerMask, LinearVelocity, PhysicsPlugins, RigidBody};
use bevy::ecs::message::Messages;
use bevy::time::TimeUpdateStrategy;

use super::march_tests::FIXTURE_88;
use super::*;
use crate::damage::{Ammo, CrewStation, KnockoutReason, TankKnockedOut};
use crate::damage_cam::recorder::{RecordedShot, ShotRecorder, ShotRecording};
use crate::damage_cam::{ConsequenceState, PenetrationReport, ShotVerdict};
use crate::spec::FireMechanism;
use crate::tank::Rig;

/// The fixed step: 64 Hz, the game's tick rate, so each update runs exactly one fixed tick.
const TICK: Duration = Duration::from_micros(15_625);
/// Fixture muzzle speed (m/s).
const SPEED: f32 = 800.0;
/// Reference-mm per metre: steel, and a box's soft substance (fixture data).
const STEEL: f32 = 1000.0;
const SOFT: f32 = 10.0;
/// A 50 mm steel plate the fixture 88 perforates.
const THIN: f32 = 0.05;
/// An HP pool no fixture shot empties, and one any shot that reaches it empties.
const DEEP: u32 = 100_000;
const ONE: u32 = 1;

#[derive(Clone, Copy)]
enum Kind {
    Plate,
    Crew(u32),
    Ammo(u32),
}

/// One fixture volume: name, full extents and centre in the tank's frame (m), kind.
type Volume = (&'static str, Vec3, Vec3, Kind);

/// The game's sim composition minus the tank asset: the march, the damage consequences and the
/// recorder, on the fixed tick with the physics step after it. `client` adds the net client's
/// replica marker.
fn recording_world(client: bool) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        PhysicsPlugins::default(),
    ))
    .init_asset::<Mesh>()
    .insert_resource(TimeUpdateStrategy::ManualDuration(TICK))
    .add_plugins((
        sim_plugin,
        crate::damage::plugin,
        crate::damage_cam::recorder::plugin,
    ));
    app.insert_resource(MgShortCircuit(false));
    if client {
        app.insert_resource(ClientReplica);
    }
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
    app
}

/// A fixture tank rooted at `at`, turned `yaw` about `+Y` and moving at `velocity`: one kinematic
/// body whose volumes are its child colliders, as a tank's are. Returns the root and each volume.
fn tank(
    app: &mut App,
    at: Vec3,
    yaw: f32,
    velocity: Vec3,
    volumes: &[Volume],
) -> (Entity, Vec<Entity>) {
    let root = app
        .world_mut()
        .spawn((
            Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw)),
            RigidBody::Kinematic,
            LinearVelocity(velocity),
        ))
        .id();
    app.world_mut().entity_mut(root).insert(Rig {
        hull: root,
        turret: root,
        gun: root,
        muzzle: root,
    });
    let spawned = volumes
        .iter()
        .map(|&(name, size, centre, kind)| {
            let factor = if matches!(kind, Kind::Plate) {
                STEEL
            } else {
                SOFT
            };
            let mut volume = app.world_mut().spawn((
                Name::new(name),
                Transform::from_translation(centre),
                ChildOf(root),
                Collider::cuboid(size.x, size.y, size.z),
                CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                BallisticVolume {
                    material: walk::VolumeMaterial::fixture(factor),
                    substance: "fixture".to_owned(),
                },
                VolumeOf(root),
            ));
            match kind {
                Kind::Plate => {}
                Kind::Crew(hp) => {
                    volume.insert((
                        ComponentHealth {
                            current: hp,
                            max: hp,
                        },
                        CrewStation::Gunner,
                    ));
                }
                Kind::Ammo(hp) => {
                    volume.insert((
                        ComponentHealth {
                            current: hp,
                            max: hp,
                        },
                        Ammo,
                    ));
                }
            }
            volume.id()
        })
        .collect();
    (root, spawned)
}

/// A plate across the line, a crew box behind it and a second box behind that, `hp` each: the
/// fixture 88 perforates the plate and crosses both boxes.
fn plated(crew: u32, rear: Kind) -> [Volume; 3] {
    [
        (
            "plate",
            Vec3::new(3.0, 3.0, THIN),
            Vec3::new(0.0, 0.0, 1.0),
            Kind::Plate,
        ),
        (
            "crew",
            Vec3::new(2.0, 2.0, 0.6),
            Vec3::new(0.0, 0.0, 0.4),
            Kind::Crew(crew),
        ),
        (
            "rear",
            Vec3::new(2.0, 2.0, 0.6),
            Vec3::new(0.0, 0.0, -0.6),
            rear,
        ),
    ]
}

/// Let the physics step register every collider before anything is fired.
fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

/// Fire the fixture 88 from `from` along `-Z` with `mechanism`, from a tank-less shooter entity.
fn fire(app: &mut App, from: Vec3, mechanism: FireMechanism) {
    let shooter = app.world_mut().spawn_empty().id();
    app.world_mut().trigger(FireShell {
        origin: from,
        direction: Dir3::NEG_Z,
        speed: SPEED,
        caliber: FIXTURE_88.caliber,
        mass: FIXTURE_88.mass,
        nose_factor: FIXTURE_88.nose_factor,
        mechanism,
        shooter: Some(ShotSource {
            tank: shooter,
            weapon: 0,
        }),
        tracer: false,
        shot_origin: FireShellOrigin::Local,
        catch_up_ticks: 0,
        shot: None,
    });
}

/// Run `ticks` fixed ticks and collect every record emitted.
fn run(app: &mut App, ticks: usize) -> Vec<RecordedShot> {
    let mut recorded = Vec::new();
    for _ in 0..ticks {
        app.update();
        recorded.extend(
            app.world_mut()
                .resource_mut::<Messages<RecordedShot>>()
                .drain(),
        );
    }
    recorded
}

fn hp(app: &App, volume: Entity) -> u32 {
    app.world().get::<ComponentHealth>(volume).unwrap().current
}

/// `report`'s loss on the component named `name`.
fn lost(report: &PenetrationReport, name: &str) -> u32 {
    report
        .doses
        .iter()
        .find(|dose| dose.volume == name)
        .map_or(0, |dose| dose.lost)
}

/// `report`'s HP at the shot for the component named `name`.
fn at_shot(report: &PenetrationReport, name: &str) -> u32 {
    report
        .components
        .iter()
        .find(|component| component.name == name)
        .unwrap()
        .hp
}

/// Two rounds into one tank in one tick: each record holds only its own loss, its HP at the shot is
/// what it found, and the losses sum to each pool's whole drop.
#[test]
fn overlapping_shots_each_record_only_their_own_loss() {
    let mut app = recording_world(false);
    let (_, volumes) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &plated(DEEP, Kind::Crew(DEEP)),
    );
    settle(&mut app);
    fire(&mut app, Vec3::new(-0.3, 0.0, 30.0), FireMechanism::Single);
    fire(&mut app, Vec3::new(0.3, 0.0, 30.0), FireMechanism::Single);
    let recorded = run(&mut app, 8);
    assert_eq!(recorded.len(), 2, "one record per round");
    for (name, volume) in [("crew", volumes[1]), ("rear", volumes[2])] {
        let drop = DEEP - hp(&app, volume);
        let losses: Vec<u32> = recorded
            .iter()
            .map(|shot| lost(&shot.report, name))
            .collect();
        assert!(
            losses.iter().all(|&loss| loss > 0),
            "{name}: both rounds reach it"
        );
        assert_eq!(
            losses.iter().sum::<u32>(),
            drop,
            "{name}: the losses are the drop"
        );
        let found: Vec<u32> = recorded
            .iter()
            .map(|shot| at_shot(&shot.report, name))
            .collect();
        let (first, second) = if found[0] > found[1] { (0, 1) } else { (1, 0) };
        assert_eq!(
            found[first], DEEP,
            "{name}: the first round found the full pool"
        );
        assert_eq!(
            found[second],
            DEEP - losses[first],
            "{name}: the second found what the first left"
        );
    }
}

/// A round through two tanks writes one record per tank, each with that tank's own components and
/// losses.
#[test]
fn a_path_through_two_tanks_records_each() {
    let mut app = recording_world(false);
    let (near, near_volumes) = tank(
        &mut app,
        Vec3::new(0.0, 0.0, 10.0),
        0.0,
        Vec3::ZERO,
        &plated(DEEP, Kind::Crew(DEEP)),
    );
    let (far, far_volumes) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &plated(DEEP, Kind::Crew(DEEP)),
    );
    settle(&mut app);
    fire(&mut app, Vec3::new(0.0, 0.0, 40.0), FireMechanism::Single);
    let recorded = run(&mut app, 8);
    assert_eq!(recorded.len(), 2, "one record per struck tank");
    for (tank, volumes) in [(near, &near_volumes), (far, &far_volumes)] {
        let shot = recorded
            .iter()
            .find(|shot| shot.tank == tank)
            .expect("each tank has its record");
        assert!(
            shot.shooter.is_some() && shot.shot.is_none(),
            "the shot's own source"
        );
        assert_eq!(shot.report.impact.verdict, ShotVerdict::Penetration);
        assert_eq!(shot.report.components.len(), 2, "its own two components");
        for (name, volume) in [("crew", volumes[1]), ("rear", volumes[2])] {
            assert_eq!(lost(&shot.report, name), DEEP - hp(&app, volume), "{name}");
            assert_eq!(at_shot(&shot.report, name), DEEP, "{name}");
        }
    }
}

/// Despawn every recorded shell that has struck armour, in the tick it struck.
fn despawn_struck(
    shells: Query<(Entity, &PenetrationMarks), With<ShotRecording>>,
    mut commands: Commands,
) {
    for (shell, marks) in &shells {
        if !marks.log.is_empty() {
            commands.entity(shell).despawn();
        }
    }
}

/// A shell despawned in the tick it struck still yields its record, the same as one left to fly
/// on; one the march itself despawns where it came to rest yields its record too.
#[test]
fn a_shell_despawned_after_the_shot_keeps_its_record() {
    let record = |despawn: bool| {
        let mut app = recording_world(false);
        if despawn {
            app.add_systems(
                FixedUpdate,
                despawn_struck
                    .after(SimPhase::ProjectileMarch)
                    .before(crate::damage::DamageConsequences),
            );
        }
        let (_, volumes) = tank(
            &mut app,
            Vec3::ZERO,
            0.0,
            Vec3::ZERO,
            &plated(DEEP, Kind::Crew(DEEP)),
        );
        settle(&mut app);
        fire(&mut app, Vec3::new(0.0, 0.0, 30.0), FireMechanism::Single);
        let mut recorded = run(&mut app, 8);
        assert_eq!(recorded.len(), 1, "one record");
        let report = recorded.pop().unwrap().report;
        assert_eq!(lost(&report, "crew"), DEEP - hp(&app, volumes[1]));
        assert!(app.world().resource::<ShotRecorder>().is_idle());
        report
    };
    assert_eq!(record(true), record(false));

    let mut app = recording_world(false);
    let mut volumes = plated(DEEP, Kind::Crew(DEEP)).to_vec();
    volumes.push((
        "back",
        Vec3::new(3.0, 3.0, 0.5),
        Vec3::new(0.0, 0.0, -1.5),
        Kind::Plate,
    ));
    tank(&mut app, Vec3::ZERO, 0.0, Vec3::ZERO, &volumes);
    settle(&mut app);
    fire(&mut app, Vec3::new(0.0, 0.0, 30.0), FireMechanism::Single);
    let recorded = run(&mut app, 8);
    assert_eq!(recorded.len(), 1, "the rested round records");
    assert!(matches!(
        recorded[0].report.end,
        crate::damage_cam::RoundEnd::Rest { .. }
    ));
    assert!(
        app.world_mut()
            .query_filtered::<(), With<ShotRecording>>()
            .iter(app.world())
            .next()
            .is_none(),
        "the march despawned it"
    );
}

/// Two rounds in one tick into a tank with one crewman: the round whose own deposition emptied the
/// crew station is the kill; the other is not. A round that empties the ammunition is a cookoff
/// kill.
#[test]
fn the_killing_shot_alone_is_attributed() {
    let mut app = recording_world(false);
    let (root, volumes) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &plated(ONE, Kind::Crew(DEEP)),
    );
    // Only one crewman decides the tank: the rear box is a module.
    app.world_mut()
        .entity_mut(volumes[2])
        .remove::<CrewStation>();
    settle(&mut app);
    fire(&mut app, Vec3::new(-0.3, 0.0, 30.0), FireMechanism::Single);
    fire(&mut app, Vec3::new(0.3, 0.0, 30.0), FireMechanism::Single);
    let recorded = run(&mut app, 8);
    assert_eq!(recorded.len(), 2);
    assert_eq!(
        app.world()
            .get::<TankKnockedOut>(root)
            .map(|label| label.reason),
        Some(KnockoutReason::CrewLoss)
    );
    let (killer, other): (Vec<_>, Vec<_>) = recorded
        .iter()
        .partition(|shot| shot.report.killed.is_some());
    assert_eq!(killer.len(), 1, "exactly one killing shot");
    let (killer, other) = (&killer[0].report, &other[0].report);
    assert_eq!(killer.killed, Some(KnockoutReason::CrewLoss));
    assert_eq!(
        lost(killer, "crew"),
        ONE,
        "the killer emptied the crew station"
    );
    assert!(
        killer
            .consequences
            .iter()
            .any(|consequence| consequence.volume == "crew"
                && consequence.state == ConsequenceState::Lost)
    );
    assert_eq!(lost(other, "crew"), 0, "the other found it empty");
    assert_eq!(at_shot(other, "crew"), 0);
    assert!(
        other
            .consequences
            .iter()
            .all(|consequence| consequence.volume != "crew")
    );

    let mut app = recording_world(false);
    let (root, _) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &plated(DEEP, Kind::Ammo(ONE)),
    );
    settle(&mut app);
    fire(&mut app, Vec3::new(0.0, 0.0, 30.0), FireMechanism::Single);
    let recorded = run(&mut app, 8);
    assert_eq!(recorded.len(), 1);
    assert!(app.world().get::<TankKnockedOut>(root).is_some());
    assert_eq!(recorded[0].report.killed, Some(KnockoutReason::Cookoff));
}

/// Automatic fire is not recorded, though it struck and dosed the tank.
#[test]
fn automatic_fire_records_nothing() {
    let mut app = recording_world(false);
    let (_, volumes) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &plated(DEEP, Kind::Crew(DEEP)),
    );
    settle(&mut app);
    fire(
        &mut app,
        Vec3::new(0.0, 0.0, 30.0),
        FireMechanism::Automatic,
    );
    assert!(run(&mut app, 8).is_empty());
    assert!(hp(&app, volumes[1]) < DEEP, "the round struck");
}

/// A net client records nothing: no shell carries a recording and no record is emitted.
#[test]
fn a_client_records_nothing() {
    let mut app = recording_world(true);
    tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &plated(DEEP, Kind::Crew(DEEP)),
    );
    settle(&mut app);
    fire(&mut app, Vec3::new(0.0, 0.0, 30.0), FireMechanism::Single);
    let mut recorded = Vec::new();
    for _ in 0..8 {
        recorded.extend(run(&mut app, 1));
        assert!(
            app.world_mut()
                .query_filtered::<(), With<ShotRecording>>()
                .iter(app.world())
                .next()
                .is_none()
        );
    }
    assert!(recorded.is_empty());
    assert!(app.world().resource::<ShotRecorder>().is_idle());
}

/// A record is stated in the hull frame the march resolved the shot against: a round fired at a
/// turned tank driving into it strikes the plate's front face in the hull frame, as it does a tank
/// at rest. A frame one tick stale would be off by the tank's travel in a tick (0.16 m, DERIVED);
/// 1 mm is the tolerance.
#[test]
fn a_record_is_stated_in_the_frame_the_march_resolved() {
    let face = |velocity: Vec3| {
        let mut app = recording_world(false);
        let yaw = 0.5_f32;
        tank(
            &mut app,
            Vec3::ZERO,
            yaw,
            velocity,
            &plated(DEEP, Kind::Crew(DEEP)),
        );
        settle(&mut app);
        fire(&mut app, Vec3::new(0.0, 0.0, 30.0), FireMechanism::Single);
        let recorded = run(&mut app, 8);
        assert_eq!(recorded.len(), 1);
        recorded[0].report.impact.point
    };
    let front = 1.0 + 0.5 * THIN;
    for velocity in [Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0)] {
        let point = face(velocity);
        assert!(
            (point[2] - front).abs() < 1.0e-3,
            "{velocity}: struck at hull z {} (the face is at {front})",
            point[2]
        );
    }
}
