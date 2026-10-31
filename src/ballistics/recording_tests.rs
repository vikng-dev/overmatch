//! The pen cam recorder in the game's fixed-tick sim ([`crate::damage_cam::recorder`]): rounds fired
//! through [`FireShell`] at generic fixture tanks, the real march, the damage consequences and the
//! physics step, read back as [`RecordedShot`]s; and one hand-built shell log read straight through
//! the record, for a pool drained between the shot's marks. Every geometry, HP pool and speed here
//! is fixture data; the assertions are class invariants.

use std::time::Duration;

use avian3d::prelude::{
    AngularInertia, CollisionLayers, LayerMask, LinearVelocity, Mass, NoAutoAngularInertia,
    NoAutoMass, PhysicsPlugins, RigidBody,
};
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
/// An HP pool no fixture shot empties, and one any shot that reaches it empties (HP points).
const DEEP: u32 = 100_000;
const ONE: u32 = 1;
/// The same pools full, in the pool units a pool and a record keep.
const DEEP_UNITS: u32 = DEEP * POOL_UNITS_PER_HP;
const ONE_UNITS: u32 = ONE * POOL_UNITS_PER_HP;

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
/// body whose volumes are its child colliders, as a tank's are, with a tank's mass (FIXTURE DATA:
/// 50 t, inertia 1e5 kg m² about each axis), so a hit's impulse leaves it in place. With the mass
/// its colliders' default density gives, the impulse throws it downrange faster than the round,
/// over the round's next start, and the round crosses it again. Returns the root and each volume.
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
            Mass(50_000.0),
            AngularInertia::new(Vec3::splat(1.0e5)),
            NoAutoMass,
            NoAutoAngularInertia,
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
                    volume.insert((ComponentHealth::full(hp), CrewStation::Gunner));
                }
                Kind::Ammo(hp) => {
                    volume.insert((ComponentHealth::full(hp), Ammo));
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
    fire_along(app, from, Dir3::NEG_Z, mechanism, None);
}

/// Fire the fixture 88 from `from` along `direction` with `mechanism` as shot `shot`, from a
/// tank-less shooter entity.
fn fire_along(
    app: &mut App,
    from: Vec3,
    direction: Dir3,
    mechanism: FireMechanism,
    shot: Option<crate::ShotId>,
) {
    fire_round(app, from, direction, mechanism, shot, None);
}

/// [`fire_along`] with the round carrying `burst`.
fn fire_round(
    app: &mut App,
    from: Vec3,
    direction: Dir3,
    mechanism: FireMechanism,
    shot: Option<crate::ShotId>,
    burst: Option<burst::Burst>,
) {
    let shooter = app.world_mut().spawn_empty().id();
    app.world_mut().trigger(FireShell {
        origin: from,
        direction,
        speed: SPEED,
        caliber: FIXTURE_88.caliber,
        mass: FIXTURE_88.mass,
        drag_k: crate::ballistics::fixture_drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
        nose_factor: FIXTURE_88.nose_factor,
        penetrator_caliber: FIXTURE_88.caliber,
        penetrator_mass: FIXTURE_88.mass,
        wound: 30.0,
        burst,
        fuze_mode: 0,
        mechanism,
        shooter: Some(ShotSource {
            tank: shooter,
            weapon: 0,
            round: 0,
        }),
        tracer: false,
        shot_origin: FireShellOrigin::Local,
        catch_up_ticks: 0,
        shot,
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

/// The pool units `volume`'s pool holds.
fn hp(app: &App, volume: Entity) -> u32 {
    app.world().get::<ComponentHealth>(volume).unwrap().held()
}

/// `report`'s loss on the component named `name`, in pool units.
fn lost(report: &PenetrationReport, name: &str) -> u32 {
    report
        .doses
        .iter()
        .find(|dose| dose.volume == name)
        .map_or(0, |dose| dose.taken)
}

/// What `report` has the pool of the component named `name` hold at the shot, in pool units.
fn at_shot(report: &PenetrationReport, name: &str) -> u32 {
    report
        .components
        .iter()
        .find(|component| component.name == name)
        .unwrap()
        .held
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
        let drop = DEEP_UNITS - hp(&app, volume);
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
            found[first], DEEP_UNITS,
            "{name}: the first round found the full pool"
        );
        assert_eq!(
            found[second],
            DEEP_UNITS - losses[first],
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
            assert_eq!(
                lost(&shot.report, name),
                DEEP_UNITS - hp(&app, volume),
                "{name}"
            );
            assert_eq!(at_shot(&shot.report, name), DEEP_UNITS, "{name}");
        }
    }
    let outcome = |tank| {
        recorded
            .iter()
            .find(|shot| shot.tank == tank)
            .unwrap()
            .outcome
    };
    assert_eq!(
        outcome(near),
        Some(0),
        "the first crossing is the end a replica draws"
    );
    assert_eq!(outcome(far), None, "past it, no replica draws an impact");
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
        assert_eq!(lost(&report, "crew"), DEEP_UNITS - hp(&app, volumes[1]));
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

/// An armed round's record stays open while the fuze the struck tank armed runs, though the round
/// is clear of the tank: it holds the burst wherever the delay ends, while its nose piece flies on.
/// The fixture 88 at 800 m/s arms in the 50 mm plate and, on a 50 ms delay (fixture data), flies
/// clear of the tank for 3 ticks and bursts about 26 m past its last box (DERIVED from the fixture
/// data); the fixture tank's mass leaves it in place under the hit's impulse. A mutant that
/// finishes the record once the round is clear, or drops the burst's field from the shell's logs,
/// fails here.
#[test]
fn an_armed_round_keeps_its_record_open_until_it_bursts() {
    let mut app = recording_world(false);
    tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &plated(DEEP, Kind::Crew(DEEP)),
    );
    settle(&mut app);
    let inputs = burst::BurstInputs {
        delay: 0.05,
        ..burst::fixture_inputs()
    };
    let burst = burst::Burst::derived(FIXTURE_88, burst::FIXTURE_STEEL, &inputs).unwrap();
    let from = Vec3::new(0.0, 0.0, 30.0);
    fire_round(
        &mut app,
        from,
        Dir3::NEG_Z,
        FireMechanism::Single,
        None,
        Some(burst),
    );
    let mut recorded = run(&mut app, 12);
    assert_eq!(recorded.len(), 1, "one record");
    let report = recorded.pop().unwrap().report;
    let held = report.burst.expect("the record holds the burst");
    assert!(
        held.point[2] < -20.0,
        "it burst clear of the tank, at {:?}",
        held.point
    );
    assert!(held.field.is_some(), "with its field");
    assert!(app.world().resource::<ShotRecorder>().is_idle());
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
        ONE_UNITS,
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
    assert!(hp(&app, volumes[1]) < DEEP_UNITS, "the round struck");
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

/// A fixture hull for the crossing tests: 20 mm plates 3 m apart at its front and back, and a crew
/// box between them that no shot here empties. FIXTURE DATA: the fixture 88 perforates every plate
/// of two such hulls.
fn hull() -> [Volume; 3] {
    let plate = Vec3::new(3.0, 3.0, 0.02);
    [
        ("front", plate, Vec3::new(0.0, 0.0, 1.5), Kind::Plate),
        (
            "crew",
            Vec3::new(2.0, 2.0, 0.6),
            Vec3::ZERO,
            Kind::Crew(DEEP),
        ),
        ("back", plate, Vec3::new(0.0, 0.0, -1.5), Kind::Plate),
    ]
}

/// Where a crossing test fires from, on the hull's axis along `-Z` (m from the near hull's centre).
/// The round flies 12.5 m a tick (DERIVED: 800 m/s × 15.625 ms), so its third tick ends 37.5 m on;
/// a hull driving at it at 10 m/s stands 0.31 m nearer by then (DERIVED), and the third tick ends
/// between its plates, 1.5 m from each.
const FROM: f32 = 37.8;

/// A fixture tank of [`hull`] at rest at `at`, carrying its envelope as a tank's spawn does
/// ([`Envelope`]): the bound of its boxes in its frame, exact for boxes square to that frame on a
/// body with no servos. Returns the root and each volume.
fn enveloped_tank(app: &mut App, at: Vec3) -> (Entity, Vec<Entity>) {
    let volumes = hull();
    let (root, spawned) = tank(app, at, 0.0, Vec3::ZERO, &volumes);
    let (min, max) = volumes.iter().fold(
        (Vec3::INFINITY, Vec3::NEG_INFINITY),
        |(min, max), &(_, size, centre, _)| {
            (min.min(centre - size * 0.5), max.max(centre + size * 0.5))
        },
    );
    app.world_mut()
        .entity_mut(root)
        .insert(Envelope(bevy::math::bounding::Aabb3d::from_min_max(
            min, max,
        )));
    (root, spawned)
}

/// One fixed tick of a crossing test: each tank's pose where the tick's march found its armour,
/// every armour read the tick fired (world position, verdict), and the travel the round owed after
/// it.
struct TickReads {
    poses: Vec<(Vec3, Quat)>,
    impacts: Vec<(Vec3, Verdict)>,
    owed: f32,
}

/// Fire the fixture 88 along `-Z` from `from` and run `ticks` fixed ticks, reading each. A tank's
/// pose is read before the tick off its front plate (each of `fronts`, square to the hull at
/// [`hull`]'s front centre): the collider the march casts against, which the physics step placed
/// from the tank's pose at that step's start, a step behind a moving body's own `Position`.
fn fire_and_read(app: &mut App, fronts: &[Entity], from: Vec3, ticks: usize) -> Vec<TickReads> {
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = log.clone();
    app.add_observer(move |impact: On<Impact>| {
        sink.lock()
            .unwrap()
            .push((impact.position, impact.outcome.verdict()));
    });
    fire(app, from, FireMechanism::Single);
    let centre = hull()[0].2;
    (0..ticks)
        .map(|_| {
            let poses = fronts
                .iter()
                .map(|&front| {
                    let world = app.world();
                    let turn = world.get::<Rotation>(front).unwrap().0;
                    (
                        world.get::<Position>(front).unwrap().0 - turn * centre,
                        turn,
                    )
                })
                .collect();
            app.update();
            let owed = app
                .world_mut()
                .query::<&Projectile>()
                .iter(app.world())
                .map(|projectile| projectile.owed)
                .sum();
            TickReads {
                poses,
                impacts: std::mem::take(&mut *log.lock().unwrap()),
                owed,
            }
        })
        .collect()
}

/// The tick the round crossed the `tank`th tank in, asserting the crossing resolved whole there
/// against the pose that tick's march found the tank's armour at: every read of the tank falls in that one tick,
/// each lies on one of its boxes' faces toward the round in its hull frame at that pose, and both
/// plates are read and perforated. A crossing split across a tick reads its later boxes against a
/// pose the tank's travel in a tick (0.16 m, DERIVED) away; 1 mm is the tolerance. Reads are
/// attributed by hull z: the fixture hulls stand metres apart.
fn crossed_whole(reads: &[TickReads], tank: usize) -> usize {
    let faces = hull().map(|(_, size, centre, _)| centre.z + size.z * 0.5);
    let hull_z = |tick: &TickReads, at: Vec3| {
        let (position, rotation) = tick.poses[tank];
        (rotation.inverse() * (at - position)).z
    };
    let ticks: Vec<usize> = (0..reads.len())
        .filter(|&i| {
            reads[i]
                .impacts
                .iter()
                .any(|&(at, _)| hull_z(&reads[i], at).abs() < 2.0)
        })
        .collect();
    let [tick] = ticks[..] else {
        let hull_reads: Vec<(usize, f32, Verdict)> = ticks
            .iter()
            .flat_map(|&i| {
                reads[i]
                    .impacts
                    .iter()
                    .map(move |&(at, verdict)| (i, hull_z(&reads[i], at), verdict))
            })
            .collect();
        panic!("tank {tank} is read in one tick, not in ticks {ticks:?}: {hull_reads:?}");
    };
    let read: Vec<(f32, Verdict)> = reads[tick]
        .impacts
        .iter()
        .map(|&(at, verdict)| (hull_z(&reads[tick], at), verdict))
        .filter(|(z, _)| z.abs() < 2.0)
        .collect();
    for &(z, _) in &read {
        assert!(
            faces.iter().any(|face| (z - face).abs() < 1.0e-3),
            "tank {tank}: a read at hull z {z}, on none of its faces {faces:?}"
        );
    }
    for plate in [faces[0], faces[2]] {
        assert!(
            read.iter()
                .any(|&(z, verdict)| (z - plate).abs() < 1.0e-3 && verdict.entered()),
            "tank {tank}: the plate at hull z {plate} is read and perforated: {read:?}"
        );
    }
    tick
}

/// A MOVING TANK'S CROSSING RESOLVES AGAINST ITS POSE AT CONTACT (design §13.5, "The envelope").
///
/// The fixture tank drives at the round at 10 m/s, and the tick that brings the round into its
/// envelope ends between its plates ([`FROM`]). The crossing still resolves whole in that tick,
/// against the pose the tick found the tank's armour at: the back plate is read where it stood at
/// contact, not where the tank has driven it by the next tick. The round leaves the tick past the
/// envelope, owing the travel it flew beyond the tick's. A march that let the tick's travel end the
/// crossing reads the back plate a tick late, 0.16 m off its face (DERIVED); one that placed the
/// envelope at the body's own pose, a step ahead of its colliders, ends the crossing short of the
/// back plate, which has driven past the round by the next tick and is never read.
#[test]
fn a_moving_tanks_crossing_resolves_against_its_pose_at_contact() {
    let mut app = recording_world(false);
    let (tank, volumes) = enveloped_tank(&mut app, Vec3::ZERO);
    settle(&mut app);
    app.world_mut()
        .entity_mut(tank)
        .insert(LinearVelocity(Vec3::new(0.0, 0.0, 10.0)));
    let reads = fire_and_read(&mut app, &[volumes[0]], Vec3::new(0.0, 0.0, FROM), 6);
    let contact = crossed_whole(&reads, 0);
    assert!(
        reads[contact].owed > 0.0,
        "the tick's travel ended inside the envelope, so the crossing ran on past it"
    );
    assert!(
        reads[contact + 1].poses[0].0.z - reads[contact].poses[0].0.z > 0.1,
        "the tank drove on after contact"
    );
    assert!(
        hp(&app, volumes[1]) < DEEP_UNITS,
        "the crew box took a dose"
    );
}

/// A ROUND THROUGH ONE TANK INTO A SECOND RESOLVES BOTH, EACH AT ITS OWN INSTANT.
///
/// Two fixture tanks on one line with disjoint envelopes, the near one driving at the round and the
/// far one away from it, each at 10 m/s. The tick that enters the near envelope ends inside it, so
/// that crossing runs on past the tick's travel and stops where the round leaves the near envelope,
/// owing the overrun; the far tank is the next tick's crossing, resolved whole against the pose
/// that tick found it at. Both are perforated plate by plate, each plate read once, and each crew
/// box takes a dose. A march that chained disjoint envelopes resolves both in one tick; one that
/// ended the far crossing where the receding tank's next pose covers the round reads its back
/// plate again a tick later.
#[test]
fn a_round_through_one_tank_into_a_second_resolves_both() {
    let mut app = recording_world(false);
    let (near, near_volumes) = enveloped_tank(&mut app, Vec3::ZERO);
    let (far, far_volumes) = enveloped_tank(&mut app, Vec3::new(0.0, 0.0, -9.0));
    settle(&mut app);
    for (tank, speed) in [(near, 10.0), (far, -10.0)] {
        app.world_mut()
            .entity_mut(tank)
            .insert(LinearVelocity(Vec3::new(0.0, 0.0, speed)));
    }
    let fronts = [near_volumes[0], far_volumes[0]];
    let reads = fire_and_read(&mut app, &fronts, Vec3::new(0.0, 0.0, FROM), 8);
    let (first, second) = (crossed_whole(&reads, 0), crossed_whole(&reads, 1));
    assert!(
        reads[first].owed > 0.0,
        "the near crossing ran past its tick's travel"
    );
    assert_eq!(
        second,
        first + 1,
        "the far tank is the next tick's crossing, at its own instant"
    );
    for (name, crew) in [("near", near_volumes[1]), ("far", far_volumes[1])] {
        assert!(
            hp(&app, crew) < DEEP_UNITS,
            "the {name} crew box took a dose"
        );
    }
}

/// A round that leaves a tank and strikes it again off another tank's armour: each encounter is its
/// own record of the one shot, their losses partition the shot's loss on the tank, and a knockout
/// the second encounter dealt is that encounter's. Fixture geometry: the round crosses the struck
/// tank's front box, glances off a thick plate on a second tank 6 m to its side at ~75° incidence
/// (past the ricochet angle), and comes back across the struck tank's rear box and the ammunition
/// behind it, clear of the struck tank's bounds for over a tick in between. The front box's soft
/// substance turns the round a hundredth of a reference plate's bend, so the return crosses the
/// tank's axis about 45 m behind the front box (DERIVED: 2 × 6 m / tan 15°).
#[test]
fn a_round_that_strikes_a_tank_again_records_each_encounter() {
    let mut app = recording_world(false);
    let wide = |name, z, kind| (name, Vec3::new(6.0, 2.0, 0.4), Vec3::new(0.0, 0.0, z), kind);
    let (struck, volumes) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[
            (
                "front",
                Vec3::new(2.0, 2.0, 0.4),
                Vec3::ZERO,
                Kind::Crew(DEEP),
            ),
            wide("rear", -45.0, Kind::Crew(DEEP)),
            wide("ammo", -46.0, Kind::Ammo(ONE)),
        ],
    );
    // Held in place: its soft volumes weigh next to nothing, so the first hit's impulse would throw
    // it out of the returning round's path.
    app.world_mut()
        .entity_mut(struck)
        .insert(avian3d::prelude::LockedAxes::ALL_LOCKED);
    tank(
        &mut app,
        Vec3::new(6.15, 0.0, -28.0),
        0.0,
        Vec3::ZERO,
        &[("glacis", Vec3::new(0.3, 3.0, 40.0), Vec3::ZERO, Kind::Plate)],
    );
    settle(&mut app);
    let shot = crate::ShotId {
        combatant: crate::CombatantId(7),
        weapon: 0,
        fire_tick: 1,
    };
    let sanctioned = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let ends = sanctioned.clone();
    app.add_observer(move |end: On<ShellTerminal>| ends.lock().unwrap().push(end.after_bounces));
    let bounces = sanctioned.clone();
    app.add_observer(move |bounce: On<ShellRicochet>| {
        bounces.lock().unwrap().push(bounce.sequence);
    });
    let graze = 15.0_f32.to_radians();
    fire_along(
        &mut app,
        Vec3::new(-30.0 * graze.tan(), 0.0, 30.0),
        Dir3::new(Vec3::new(graze.sin(), 0.0, -graze.cos())).unwrap(),
        FireMechanism::Single,
        Some(shot),
    );
    let recorded = run(&mut app, 16);
    let mut encounters: Vec<&RecordedShot> = recorded
        .iter()
        .filter(|record| record.tank == struck)
        .collect();
    encounters.sort_by_key(|record| record.encounter);
    assert_eq!(
        encounters
            .iter()
            .map(|record| record.encounter)
            .collect::<Vec<_>>(),
        vec![0, 1],
        "two encounters with the struck tank"
    );
    assert!(encounters.iter().all(|record| record.shot == Some(shot)));
    assert_eq!(
        *sanctioned.lock().unwrap(),
        vec![0],
        "the authority sanctions the first crossing as the end, and nothing after it"
    );
    assert_eq!(
        encounters
            .iter()
            .map(|record| record.outcome)
            .collect::<Vec<_>>(),
        vec![Some(0), None],
        "the first encounter is drawn at the sanctioned end; the return, past it, is not drawn"
    );
    let (first, second) = (&encounters[0].report, &encounters[1].report);

    // The shot's own tally: the cookoff then empties every pool, so the pools' drop is not the
    // shot's.
    let tally: Vec<(Entity, u32)> = app
        .world_mut()
        .query_filtered::<&ShotDose, With<ShotRecording>>()
        .single(app.world())
        .expect("the round flies on")
        .losses()
        .collect();
    for (name, volume) in ["front", "rear", "ammo"].into_iter().zip(&volumes) {
        let shot_loss = tally
            .iter()
            .find(|(box_, _)| box_ == volume)
            .map_or(0, |(_, lost)| *lost);
        assert_eq!(
            lost(first, name) + lost(second, name),
            shot_loss,
            "{name}: the encounters partition the shot's loss"
        );
    }
    assert!(
        lost(first, "front") > 0,
        "the first encounter crossed the front"
    );
    assert_eq!(
        lost(second, "front"),
        0,
        "and the second does not count it again"
    );
    assert_eq!(
        at_shot(second, "front"),
        DEEP_UNITS - lost(first, "front"),
        "the second encounter found what the first left"
    );
    assert!(lost(second, "rear") > 0);
    assert_eq!((lost(first, "ammo"), lost(second, "ammo")), (0, ONE_UNITS));

    assert_eq!(first.killed, None);
    assert_eq!(second.killed, Some(KnockoutReason::Cookoff));
    assert!(first.consequences.is_empty());
    assert!(
        second
            .consequences
            .iter()
            .any(|consequence| consequence.volume == "ammo"
                && consequence.state == ConsequenceState::Cookoff)
    );
    assert!(app.world().resource::<ShotRecorder>().is_idle());
}

/// An emptying is timed at the deposit that took the pool's last point, though another round
/// drained the pool between the shot's marks: the shot doses an ammunition box at one mark, the
/// pool is drained, and at the next mark a spall cone's dose empties it before the line reaches
/// it. Fixture log and doses (J), hull frame = world.
#[test]
fn an_emptying_is_timed_at_the_deposit_that_emptied_the_pool() {
    use crate::damage_cam::record::{self, HullFrame, ShotFacts, ShotLog, StruckTank};
    use bevy::ecs::system::RunSystemOnce;
    use std::collections::BTreeMap;

    let mut world = World::new();
    let tank = world.spawn_empty().id();
    let ammo = world
        .spawn((
            Name::new("ammo"),
            VolumeOf(tank),
            ComponentHealth::full(100),
            Ammo,
        ))
        .id();
    let plate = world.spawn((Name::new("plate"), VolumeOf(tank))).id();
    let shell = world.spawn(ShotDose::default()).id();
    // The line's energy (J) for 40 points at mark 0; the cone's energy for 20 points and the line's
    // for 30 at mark 1, each taken into the tally at the one energy rate.
    let joules = |points: f32| points * super::energy_dose::joules_per_hp() as f32;
    let (line_0, cone, line_1) = (joules(40.0), joules(20.0), joules(30.0));
    let dose = |world: &mut World, units: Vec<u64>, mark: usize| {
        world
            .run_system_once(
                move |mut shot: Query<&mut ShotDose>, mut health: Query<&mut ComponentHealth>| {
                    let mut shot = shot.single_mut().unwrap();
                    let context = DoseContext::Line {
                        entry: Vec3::ZERO,
                        along: Vec3::NEG_Z,
                    };
                    for units in &units {
                        shot.add_units(ammo, *units, context);
                    }
                    shot.settle(&mut health, true, mark);
                },
            )
            .unwrap();
    };
    dose(&mut world, vec![energy_units(line_0)], 0);
    // Another round leaves 10 of the 60 points.
    world
        .get_mut::<ComponentHealth>(ammo)
        .unwrap()
        .set_held(10 * POOL_UNITS_PER_HP);
    dose(
        &mut world,
        vec![energy_units(cone), energy_units(line_1)],
        1,
    );
    assert!(world.get::<ComponentHealth>(ammo).unwrap().is_empty());

    let crossing =
        |entrance: Vec3, volumes: Vec<VolumeCrossing>, exit: Vec3, cones| ArmourMark::Crossing {
            entrance,
            normal: Vec3::Z,
            incoming: -Vec3::Z,
            speed_in: SPEED,
            overmatched: false,
            volumes,
            end: CrossingEnd::Exit {
                at: exit,
                speed: SPEED,
            },
            cones,
        };
    let through = |volume, entry: Vec3, end: Vec3, energy| VolumeCrossing {
        volume,
        entry,
        end,
        chord: entry.distance(end),
        cost: 1000.0 * entry.distance(end),
        energy,
        perforated: true,
    };
    let z = |z: f32| Vec3::new(0.0, 0.0, z);
    let marks = vec![
        crossing(
            z(10.0),
            vec![through(ammo, z(10.0), z(9.0), line_0)],
            z(9.0),
            0,
        ),
        crossing(
            z(5.0),
            vec![
                through(plate, z(5.0), z(4.9), joules(100.0)),
                through(ammo, z(4.0), z(3.0), line_1),
            ],
            z(3.0),
            1,
        ),
    ];
    let cones = vec![SpallCone {
        origin: z(4.9),
        axis: Dir3::NEG_Z,
        field: super::spall_cone::FieldDescriptor::ball(2.0),
        spend: 100.0,
        energy: joules(100.0),
        entry_speed: SPEED,
        exit_speed: SPEED,
        strength: cone,
        rays: vec![SpallRay {
            end: z(3.5),
            strength: cone,
            doses: vec![(ammo, cone)],
        }],
    }];
    let struck = StruckTank::recorded(
        tank,
        0,
        vec![(0, HullFrame::posed(Vec3::ZERO, Quat::IDENTITY))],
        BTreeMap::new(),
        vec![(ammo, 100 * POOL_UNITS_PER_HP)],
    );
    let facts = ShotFacts {
        launch: z(20.0),
        ..ShotFacts::default()
    };
    let path = [z(20.0), z(9.0), z(3.0)];
    let report = record::report_from(
        &world,
        &struck,
        &ShotLog {
            marks: &marks,
            cones: &cones,
            burst: None,
            path: &path,
            dose: world.get::<ShotDose>(shell),
        },
        &facts,
    )
    .unwrap();
    assert_eq!(
        lost(&report, "ammo"),
        50 * POOL_UNITS_PER_HP,
        "40, then the 10 left"
    );
    let [cookoff] = report.consequences.as_slice() else {
        panic!("one consequence: {:?}", report.consequences);
    };
    assert_eq!(cookoff.state, ConsequenceState::Cookoff);
    assert_eq!(
        cookoff.t, report.spall[0].t,
        "the cone's dose emptied the drained pool"
    );
    // The line enters the box 1 m past the mark's entrance, at full speed.
    let entrance = report
        .path
        .iter()
        .find(|node| node.point == [0.0, 0.0, 5.0])
        .unwrap();
    assert!(
        cookoff.t < entrance.t + 1.0 / SPEED,
        "before the line reached the box"
    );
}

/// A record names the struck tank's life, the [`crate::ShotClock`] ticks its encounter opened and
/// finished on, and the hull's pose when it opened; a tank no match spawned has no life.
#[test]
fn a_record_names_the_struck_life_its_ticks_and_the_hull_pose() {
    let yaw = 0.5_f32;
    let mut app = recording_world(false);
    let (root, _) = tank(
        &mut app,
        Vec3::ZERO,
        yaw,
        Vec3::ZERO,
        &plated(DEEP, Kind::Crew(DEEP)),
    );
    app.world_mut()
        .entity_mut(root)
        .insert((crate::CombatantId(9), crate::CombatantLife(2)));
    settle(&mut app);
    fire(&mut app, Vec3::new(0.0, 0.0, 30.0), FireMechanism::Single);
    let first = 500;
    let mut recorded = Vec::new();
    for tick in first..first + 8 {
        app.insert_resource(crate::ShotClock(tick));
        recorded.extend(run(&mut app, 1));
    }
    assert_eq!(recorded.len(), 1);
    let record = &recorded[0];
    assert_eq!(
        record.life,
        Some(crate::damage_cam::wire::TankLife {
            combatant: crate::CombatantId(9),
            life: crate::CombatantLife(2),
        })
    );
    assert!(
        first <= record.contact_tick
            && record.contact_tick <= record.completion_tick
            && record.completion_tick < first + 8,
        "contact {} completion {}",
        record.contact_tick,
        record.completion_tick
    );
    let (at, turn) = record.hull.expect("the hull's pose at contact");
    assert!(at.length() < 1.0e-3, "{at}");
    assert!(turn.angle_between(Quat::from_rotation_y(yaw)) < 1.0e-3);

    let mut app = recording_world(false);
    tank(
        &mut app,
        Vec3::ZERO,
        yaw,
        Vec3::ZERO,
        &plated(DEEP, Kind::Crew(DEEP)),
    );
    settle(&mut app);
    fire(&mut app, Vec3::new(0.0, 0.0, 30.0), FireMechanism::Single);
    let recorded = run(&mut app, 8);
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].life, None);
    assert_eq!(
        (recorded[0].contact_tick, recorded[0].completion_tick),
        (0, 0)
    );
}

/// In the game's damage model a record counts the shown points its shot took settle by settle, as
/// the authority's hit confirmation does ([`ShotDose::shown_losses`]), though another round dosed
/// the pool between the shot's marks: the shot's second settle takes the pool from 3 shown points
/// to 2, so the record is dosed and carries the point. The pen cam's verdict relies on it to agree
/// with the hit marker; a record that reads what the first settle found less the shot's whole take
/// (3000 less 800 pool units, still 3 shown) loses the point and the verdict. Fixture: a 3-point
/// pool, the shot dosing 0.4 HP at each of two marks and another round 0.4 HP between them.
#[test]
fn interleaved_fractional_doses_record_the_shown_point_the_shot_took() {
    use crate::damage_cam::record::{self, HullFrame, ShotFacts, ShotLog, StruckTank};
    use bevy::ecs::system::RunSystemOnce;
    use std::collections::BTreeMap;

    let mut world = World::new();
    let tank = world.spawn_empty().id();
    let target = world
        .spawn((
            Name::new("fixture box"),
            VolumeOf(tank),
            ComponentHealth::full(3),
        ))
        .id();
    let shell = world.spawn(ShotDose::default()).id();
    for mark in 0..2 {
        world
            .run_system_once(
                move |mut shots: Query<&mut ShotDose>, mut health: Query<&mut ComponentHealth>| {
                    let mut shot = shots.single_mut().unwrap();
                    shot.add(
                        target,
                        0.04,
                        DoseContext::Line {
                            entry: Vec3::ZERO,
                            along: Vec3::NEG_Z,
                        },
                    );
                    shot.settle(&mut health, true, mark);
                },
            )
            .unwrap();
        if mark == 0 {
            world.get_mut::<ComponentHealth>(target).unwrap().take(400);
        }
    }
    let shot = world.get::<ShotDose>(shell).unwrap();
    assert_eq!(shot.shown_losses().collect::<Vec<_>>(), vec![(target, 1)]);
    let marks: Vec<_> = (0..2)
        .map(|mark| {
            let entry = Vec3::new(0.0, 0.0, -(mark as f32));
            ArmourMark::Wound {
                incoming: Vec3::NEG_Z,
                speed: SPEED,
                volumes: vec![VolumeCrossing {
                    volume: target,
                    entry,
                    end: entry + Vec3::NEG_Z * 0.4,
                    chord: 0.4,
                    cost: 0.04,
                    energy: 0.0,
                    perforated: true,
                }],
            }
        })
        .collect();
    let struck = StruckTank::recorded(
        tank,
        0,
        vec![(0, HullFrame::posed(Vec3::ZERO, Quat::IDENTITY))],
        BTreeMap::new(),
        vec![(target, 3 * POOL_UNITS_PER_HP)],
    );
    let report = record::report_from(
        &world,
        &struck,
        &ShotLog {
            marks: &marks,
            cones: &[],
            burst: None,
            path: &[Vec3::Z, Vec3::NEG_Z * 1.4],
            dose: Some(shot),
        },
        &ShotFacts {
            launch: Vec3::Z,
            ..ShotFacts::default()
        },
    )
    .expect("the shot took a shown point, so its path is dosed and it has a record");
    assert_eq!(report.steps(), vec![crate::damage_cam::PathStep::Dosed]);
    assert_eq!(report.impact.verdict, ShotVerdict::Damaged);
    assert_eq!(lost(&report, "fixture box"), 800);
    assert_eq!(report.doses[0].shown_taken, Some(1));
}
