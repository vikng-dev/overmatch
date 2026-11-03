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

/// The game's damage-model fixture compositions. The walk refreshes children after physics in
/// both; the networked composition also refreshes after the fixed loop for its render consumers.
#[derive(Clone, Copy, Debug)]
enum ChildPose {
    /// Avian without a network after-loop refresh.
    Avian,
    /// The Lightyear after-loop child refresh used by the networked server and client.
    Networked,
}

/// The game's sim composition minus the tank asset: the march, the damage consequences and the
/// recorder, on the fixed tick after the physics step. `client` adds the net client's
/// replica marker.
fn recording_world(client: bool) -> App {
    recording_world_under(client, ChildPose::Avian)
}

/// [`recording_world`], its child colliders placed as `pose` places them.
fn recording_world_under(client: bool, pose: ChildPose) -> App {
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
    if client {
        app.insert_resource(ClientReplica);
    }
    if let ChildPose::Networked = pose {
        app.add_systems(
            RunFixedMainLoop,
            lightyear::avian3d::plugin::LightyearAvianPlugin::update_child_collider_position
                .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
        );
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
    // The one knockout rule wants two crew members in action (`damage::knockout_from_counts`).
    // Seats no round reaches make up the count: one beside listed crew boxes, two with none, so a
    // fixture tank is knocked out exactly when every crew box it lists has its HP pool emptied, or
    // a cook-off empties every pool, seats included.
    let listed = volumes
        .iter()
        .filter(|&&(.., kind)| matches!(kind, Kind::Crew(_)))
        .count();
    for seat in 0..if listed == 0 { 2 } else { 1 } {
        app.world_mut().spawn((
            Name::new(format!("seat {seat}")),
            ComponentHealth::full(1),
            CrewStation::Loader,
            VolumeOf(root),
        ));
    }
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
    let fire = fixture_fire(app, from, direction, mechanism, shot, burst);
    app.world_mut().trigger(fire);
}

/// The event [`fire_round`] triggers, for a test that also sets its speed or drag.
fn fixture_fire(
    app: &mut App,
    from: Vec3,
    direction: Dir3,
    mechanism: FireMechanism,
    shot: Option<crate::ShotId>,
    burst: Option<burst::Burst>,
) -> FireShell {
    let shooter = app.world_mut().spawn_empty().id();
    FireShell {
        effects: crate::rounds::FlightEffects::fixture_traced(),
        muzzle: crate::guns::MuzzleEmission::fixture_cannon(),
        birth: from,
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
        contact: crate::rounds::ContactPreset::fixture(),
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
    }
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
    // FIXTURE DATA: distinct combatant lives identify each record through its production reader.
    for (root, id) in [(near, 90), (far, 91)] {
        app.world_mut()
            .entity_mut(root)
            .insert((crate::CombatantId(id), crate::CombatantLife(0)));
    }
    settle(&mut app);
    fire(&mut app, Vec3::new(0.0, 0.0, 40.0), FireMechanism::Single);
    let recorded = run(&mut app, 8);
    assert_eq!(recorded.len(), 2, "one record per struck tank");
    let life_of = |tank| {
        Some(crate::damage_cam::wire::TankLife {
            combatant: *app.world().get::<crate::CombatantId>(tank).unwrap(),
            life: *app.world().get::<crate::CombatantLife>(tank).unwrap(),
        })
    };
    for (tank, volumes) in [(near, &near_volumes), (far, &far_volumes)] {
        let shot = recorded
            .iter()
            .find(|shot| shot.life == life_of(tank))
            .expect("each tank has its record");
        assert!(shot.shot.is_none(), "the fixture shot is unkeyed");
        assert_eq!(shot.report.impact.verdict, ShotVerdict::Penetration);
        assert_eq!(
            shot.report.components.len(),
            3,
            "its own two boxes and the fixture's seat"
        );
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
            .find(|shot| shot.life == life_of(tank))
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

/// The game's damage model: a crossing shared by overlapping tanks opens each tank's encounter
/// when the round reaches that tank, not at the crossing's entrance. The crossing enters A late in
/// a tick and reaches B in the next; B is knocked out between, so the round strikes a husk and B
/// opens no record, while A's encounter, opened on a living target, still records. Mutant: opening
/// every tank of a crossing at its entrance records B. Dimensions, the firing distance (the
/// entrance about 1 m before a tick boundary, B 2.5 m into the crossing) and pools are fixture data.
#[test]
fn a_shared_crossing_opens_each_tank_when_the_round_reaches_it() {
    let mut app = recording_world(false);
    let (a, _) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[("a", Vec3::new(2.0, 2.0, 3.0), Vec3::ZERO, Kind::Crew(DEEP))],
    );
    let (b, _) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[(
            "b",
            Vec3::new(2.0, 2.0, 0.6),
            Vec3::new(0.0, 0.0, -1.3),
            Kind::Crew(DEEP),
        )],
    );
    for (root, id) in [(a, 92), (b, 93)] {
        app.world_mut()
            .entity_mut(root)
            .insert((crate::CombatantId(id), crate::CombatantLife(0)));
    }
    settle(&mut app);
    fire(&mut app, Vec3::new(0.0, 0.0, 25.5), FireMechanism::Single);
    let mut recorded = run(&mut app, 2);
    let mut marks = app.world_mut().query::<&PenetrationMarks>();
    let reached_a = |marks: &PenetrationMarks| {
        marks
            .log
            .iter()
            .any(|mark| matches!(mark, ArmourMark::Crossing { .. }))
    };
    assert!(
        marks.iter(app.world()).any(reached_a),
        "fixture: the crossing entered A in the second tick"
    );
    let mut shells = app
        .world_mut()
        .query_filtered::<&Transform, With<Projectile>>();
    let z = shells.single(app.world()).unwrap().translation.z;
    assert!(z > -1.0, "fixture: the round has not reached B ({z})");
    app.world_mut().entity_mut(b).insert(TankKnockedOut {
        reason: KnockoutReason::CrewLoss,
    });
    recorded.extend(run(&mut app, 8));
    let life = |id| {
        Some(crate::damage_cam::wire::TankLife {
            combatant: crate::CombatantId(id),
            life: crate::CombatantLife(0),
        })
    };
    assert!(
        recorded.iter().any(|shot| shot.life == life(92)),
        "A was living when the round reached it"
    );
    assert!(
        !recorded.iter().any(|shot| shot.life == life(93)),
        "B was a husk when the round reached it"
    );
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
                FixedPostUpdate,
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

/// The game's damage model: only a target alive at the strike opens an encounter. A husk
/// records nothing with its turret present or absent, while the knockout shot keeps its record.
/// The remaining ammunition HP pool still takes dose in every case; the record gate must not
/// change simulation. The same gate counts the husk strike for the server's stats line
/// (`ShotRecorder::husk_strikes`); a strike on a living target is not one. The servo slots,
/// geometry and HP capacities are fixture data.
#[test]
fn a_round_into_a_husk_records_nothing_with_or_without_its_turret() {
    use crate::tank::{ServoIndex, TankRoot, TankServos};
    let record = |husk: bool, turret_present: bool| {
        let mut app = recording_world(false);
        let (root, volumes) = tank(
            &mut app,
            Vec3::ZERO,
            0.0,
            Vec3::ZERO,
            &plated(ONE, Kind::Ammo(DEEP)),
        );
        let world = app.world_mut();
        world.entity_mut(root).insert(TankServos::for_count(2));
        world.spawn((Name::new("gun"), ServoIndex(0), TankRoot(root)));
        if turret_present {
            world.spawn((Name::new("turret"), ServoIndex(1), TankRoot(root)));
        }
        if husk {
            world
                .get_mut::<ComponentHealth>(volumes[1])
                .unwrap()
                .drain();
            world.entity_mut(root).insert(TankKnockedOut {
                reason: KnockoutReason::CrewLoss,
            });
        }
        settle(&mut app);
        fire(&mut app, Vec3::new(0.0, 0.0, 30.0), FireMechanism::Single);
        let recorded = run(&mut app, 8);
        assert!(app.world().resource::<ShotRecorder>().is_idle());
        assert!(
            hp(&app, volumes[2]) < DEEP_UNITS,
            "the husk's remaining pool still takes dose"
        );
        let husk_strikes = app.world().resource::<ShotRecorder>().husk_strikes();
        assert_eq!(
            husk_strikes > 0,
            husk,
            "a husk strike is counted, a living target's is not ({husk_strikes})"
        );
        recorded
    };
    let knockout = record(false, true);
    assert_eq!(knockout.len(), 1, "the knockout shot records");
    assert_eq!(knockout[0].report.killed, Some(KnockoutReason::CrewLoss));
    let posed: Vec<&str> = knockout[0]
        .report
        .servo_deg
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        posed,
        ["gun", "turret"],
        "the knockout shot keeps its strike pose"
    );
    for turret_present in [true, false] {
        assert!(
            record(true, turret_present).is_empty(),
            "a husk opens no record"
        );
    }
}

/// An armed round's record stays open while the fuze the struck tank armed runs, though the round
/// is clear of the tank: it holds the burst wherever the delay ends, while its nose piece flies on.
/// The fixture 88 at 800 m/s arms in the 50 mm plate and, on a 50 ms delay (fixture data), flies
/// clear of the tank before its delay ends; the arming threshold reads material-weighted real chord.
/// The fixture tank's mass leaves it in place under the hit's impulse. A mutant that
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
        trigger: crate::rounds::fuze::Trigger::SpendExit {
            arming_spend: THIN * STEEL * 0.5,
        },
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
    // Fixture staffing: only the front crew-station HP pool decides the knockout; the rear box is a module.
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

/// The game's match score from a real round: a keyed round whose own deposition empties the one
/// crew station of combatant B's tank credits its combatant A one knockout and B one loss, and
/// opens B's respawn for that life the mode's wait after the knockout tick; the same round fired
/// as B's own is a loss with no credit. The scoreboard, the server's respawn gate and the client's
/// countdown rely on this credit. Mutants: crediting the victim, crediting nobody for another's
/// round, crediting its own round, or a loss not counted fail. Fixture combatants and wait.
#[test]
fn a_round_from_a_knocks_out_b_crediting_a_a_knockout_and_b_a_loss() {
    use crate::net::score::{Respawn, RespawnWait, Scoreboard, Standing};
    use crate::{CombatantId, CombatantLife, ShotId};

    let (a, b) = (CombatantId(1), CombatantId(2));
    let wait = RespawnWait::new(Duration::from_secs(1), TICK);
    let scored = |by: CombatantId| -> Vec<Standing> {
        let mut app = recording_world(false);
        crate::net::score::mount(&mut app, wait);
        app.init_resource::<crate::ShotClock>();
        app.world_mut().spawn((
            Scoreboard::default(),
            crate::net::battle::BattleState::combat_fixture(),
        ));
        app.world_mut().spawn((a, CombatantLife(0)));
        let (root, volumes) = tank(
            &mut app,
            Vec3::ZERO,
            0.0,
            Vec3::ZERO,
            &plated(ONE, Kind::Crew(DEEP)),
        );
        // Fixture staffing: only the front crew-station HP pool decides the knockout; the rear box is a module.
        app.world_mut()
            .entity_mut(volumes[2])
            .remove::<CrewStation>();
        app.world_mut()
            .entity_mut(root)
            .insert((b, CombatantLife(0)));
        settle(&mut app);
        let shot = ShotId {
            combatant: by,
            weapon: 0,
            fire_tick: 1,
        };
        fire_along(
            &mut app,
            Vec3::new(0.0, 0.0, 30.0),
            Dir3::NEG_Z,
            FireMechanism::Single,
            Some(shot),
        );
        run(&mut app, 8);
        assert!(app.world().get::<TankKnockedOut>(root).is_some());
        let mut boards = app.world_mut().query::<&Scoreboard>();
        boards
            .single(app.world())
            .expect("the fixture's one scoreboard")
            .0
            .clone()
    };
    let opened = Respawn::Opens {
        life: CombatantLife(0),
        tick: crate::sim_tick::SimTick::new(wait.ticks()),
    };
    assert_eq!(
        scored(a),
        [
            Standing {
                combatant: a,
                knockouts: 1,
                losses: 0,
                respawn: Respawn::NeverKnockedOut,
            },
            Standing {
                combatant: b,
                knockouts: 0,
                losses: 1,
                respawn: opened,
            },
        ],
        "A's round knocked out B's tank"
    );
    assert_eq!(
        scored(b),
        [
            Standing {
                combatant: a,
                knockouts: 0,
                losses: 0,
                respawn: Respawn::NeverKnockedOut,
            },
            Standing {
                combatant: b,
                knockouts: 0,
                losses: 1,
                respawn: opened,
            },
        ],
        "B's own round credits nobody"
    );
}

/// The game's damage model: independent marker insertions and attributed facts from the shipped
/// consequence/recorder plugins. Combatant identities and the wait below are fixture data.
#[derive(Resource, Default)]
struct KnockoutFacts {
    transitions: Vec<Entity>,
    attributed: Vec<(Entity, crate::damage::KnockoutCause)>,
}

fn scoring_world() -> (App, Entity, Entity) {
    use crate::net::score::{RespawnWait, Scoreboard};
    use crate::{CombatantId, CombatantLife};

    let mut app = recording_world(false);
    crate::net::score::mount(&mut app, RespawnWait::new(Duration::from_secs(1), TICK));
    app.init_resource::<crate::ShotClock>()
        .init_resource::<KnockoutFacts>()
        .add_observer(
            |add: On<Add, TankKnockedOut>, mut facts: ResMut<KnockoutFacts>| {
                facts.transitions.push(add.entity);
            },
        )
        .add_observer(
            |event: On<crate::damage::TankKnockout>, mut facts: ResMut<KnockoutFacts>| {
                facts.attributed.push((event.tank, event.cause));
            },
        );
    let board = app
        .world_mut()
        .spawn((
            Scoreboard::default(),
            crate::net::battle::BattleState::combat_fixture(),
        ))
        .id();
    let shooter = app
        .world_mut()
        .spawn((CombatantId(1), CombatantLife(0)))
        .id();
    app.world_mut().spawn((CombatantId(3), CombatantLife(0)));
    (app, board, shooter)
}

/// The game's damage model: scoring fixtures use the shipped crew representation (ADR-0040),
/// so a round's crew line rate decides their dose independently of hard-body energy loss.
fn scoring_witnesses(app: &mut App, boxes: &[Entity]) {
    for &volume in boxes {
        if app.world().get::<CrewStation>(volume).is_some() {
            app.world_mut()
                .entity_mut(volume)
                .remove::<BallisticVolume>()
                .insert((
                    WitnessVolume {
                        substance: "fixture".to_owned(),
                    },
                    WitnessSurface,
                    CollisionLayers::new([Layer::Witness], LayerMask::NONE),
                ));
        }
    }
}

/// The game's damage model: the scoreboard must credit automatic fire even though the pen cam
/// records none of it. A fixture machine-gun round crosses an unarmoured crew box; the round's
/// own dose decides the knockout through the production march and consequence chain.
#[test]
fn an_unrecorded_machine_gun_round_credits_its_shooter() {
    use crate::net::score::Scoreboard;
    use crate::{CombatantId, CombatantLife, ShotId};

    let (mut app, board, _) = scoring_world();
    let (root, boxes) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[("crew", Vec3::ONE, Vec3::ZERO, Kind::Crew(ONE))],
    );
    app.world_mut()
        .entity_mut(root)
        .insert((CombatantId(2), CombatantLife(0)));
    scoring_witnesses(&mut app, &boxes);
    settle(&mut app);
    let shot = ShotId {
        combatant: CombatantId(1),
        weapon: 1,
        fire_tick: 1,
    };
    let body = super::march_tests::FIXTURE_MG;
    let mut fire = fixture_fire(
        &mut app,
        Vec3::new(0.0, 0.0, 30.0),
        Dir3::NEG_Z,
        FireMechanism::Automatic,
        Some(shot),
        None,
    );
    fire.caliber = body.caliber;
    fire.mass = body.mass;
    fire.nose_factor = body.nose_factor;
    fire.penetrator_caliber = body.caliber;
    fire.penetrator_mass = body.mass;
    fire.drag_k = fixture_drag_k(body.caliber, body.mass);
    app.world_mut().trigger(fire);
    assert!(
        run(&mut app, 8).is_empty(),
        "automatic fire opens no pen cam record"
    );
    let facts = app.world().resource::<KnockoutFacts>();
    assert_eq!(facts.transitions, [root]);
    assert_eq!(
        facts.attributed,
        [(root, crate::damage::KnockoutCause::Round(shot))]
    );
    let standings = &app.world().get::<Scoreboard>(board).unwrap().0;
    assert_eq!(
        standings
            .iter()
            .find(|s| s.combatant == CombatantId(1))
            .unwrap()
            .knockouts,
        1
    );
    assert_eq!(
        standings
            .iter()
            .find(|s| s.combatant == CombatantId(2))
            .unwrap()
            .losses,
        1
    );
}

/// The game's damage model: a later round can cause a cook-off on a crew-loss husk, but the
/// scoreboard and pen cam must retain the first round's one knockout. The offset ammunition
/// box is fixture geometry keeping the two deciding depositions in separate shots.
#[test]
fn a_later_cookoff_on_a_crew_loss_husk_cannot_credit_a_second_knockout() {
    use crate::net::score::Scoreboard;
    use crate::{CombatantId, CombatantLife, ShotId};

    let (mut app, board, _) = scoring_world();
    let mut volumes = plated(ONE, Kind::Ammo(ONE));
    volumes[2].2.x = 4.0;
    let (root, boxes) = tank(&mut app, Vec3::ZERO, 0.0, Vec3::ZERO, &volumes);
    app.world_mut()
        .entity_mut(root)
        .insert((CombatantId(2), CombatantLife(0)));
    scoring_witnesses(&mut app, &boxes);
    settle(&mut app);
    let shot = ShotId {
        combatant: CombatantId(1),
        weapon: 0,
        fire_tick: 1,
    };
    fire_along(
        &mut app,
        Vec3::new(0.0, 0.0, 30.0),
        Dir3::NEG_Z,
        FireMechanism::Single,
        Some(shot),
    );
    let records = run(&mut app, 8);
    assert_eq!(
        app.world().get::<TankKnockedOut>(root).unwrap().reason,
        KnockoutReason::CrewLoss
    );
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].report.killed, Some(KnockoutReason::CrewLoss));
    assert!(
        hp(&app, boxes[2]) > 0,
        "the first shot leaves ammunition in action"
    );
    let before = app.world().get::<Scoreboard>(board).unwrap().clone();
    fire_along(
        &mut app,
        Vec3::new(4.0, 0.0, 30.0),
        Dir3::NEG_Z,
        FireMechanism::Single,
        Some(ShotId {
            combatant: CombatantId(3),
            fire_tick: 2,
            ..shot
        }),
    );
    assert!(
        run(&mut app, 8).is_empty(),
        "a husk opens no second pen cam record"
    );
    assert!(
        app.world()
            .get::<crate::damage::CookedOff>(boxes[2])
            .is_some(),
        "the later real round must actually cause a cook-off"
    );
    assert_eq!(*app.world().get::<Scoreboard>(board).unwrap(), before);
    let facts = app.world().resource::<KnockoutFacts>();
    assert_eq!(facts.transitions, [root]);
    assert_eq!(
        facts.attributed,
        [(root, crate::damage::KnockoutCause::Round(shot))]
    );
}

/// The game's damage model: score belongs to combatants across lives. Respawning a shooter
/// while its real round is in flight must preserve its credit, and replacing the knocked-out
/// victim must retain that earlier credit when a different combatant decides its next knockout.
#[test]
fn respawns_preserve_in_flight_and_earlier_credits() {
    use crate::net::score::Scoreboard;
    use crate::{CombatantId, CombatantLife, ShotId};

    let (mut app, board, shooter) = scoring_world();
    let target = |app: &mut App, life| {
        let (root, boxes) = tank(
            app,
            Vec3::ZERO,
            0.0,
            Vec3::ZERO,
            &[("crew", Vec3::ONE, Vec3::ZERO, Kind::Crew(ONE))],
        );
        app.world_mut()
            .entity_mut(root)
            .insert((CombatantId(2), CombatantLife(life)));
        scoring_witnesses(app, &boxes);
        root
    };
    let root = target(&mut app, 0);
    settle(&mut app);
    let shot = ShotId {
        combatant: CombatantId(1),
        weapon: 0,
        fire_tick: 1,
    };
    let mut fire = fixture_fire(
        &mut app,
        Vec3::new(0.0, 0.0, 30.0),
        Dir3::NEG_Z,
        FireMechanism::Single,
        Some(shot),
        None,
    );
    fire.shooter.as_mut().unwrap().tank = shooter;
    app.world_mut().trigger(fire);
    app.world_mut().despawn(shooter);
    app.world_mut().spawn((CombatantId(1), CombatantLife(1)));
    run(&mut app, 8);
    assert!(app.world().get::<TankKnockedOut>(root).is_some());
    let before = app.world().get::<Scoreboard>(board).unwrap().clone();
    assert_eq!(
        before
            .0
            .iter()
            .find(|s| s.combatant == CombatantId(1))
            .unwrap()
            .knockouts,
        1
    );
    app.world_mut().despawn(root);
    let fresh = target(&mut app, 1);
    settle(&mut app);
    assert_eq!(*app.world().get::<Scoreboard>(board).unwrap(), before);
    fire_along(
        &mut app,
        Vec3::new(0.0, 0.0, 30.0),
        Dir3::NEG_Z,
        FireMechanism::Single,
        Some(ShotId {
            combatant: CombatantId(3),
            fire_tick: 2,
            ..shot
        }),
    );
    run(&mut app, 8);
    assert!(app.world().get::<TankKnockedOut>(fresh).is_some());
    let standings = &app.world().get::<Scoreboard>(board).unwrap().0;
    assert_eq!(
        standings.len(),
        3,
        "replacement lives do not open duplicate standings"
    );
    for (who, knockouts, losses) in [(1, 1, 0), (2, 0, 2), (3, 1, 0)] {
        let standing = standings
            .iter()
            .find(|s| s.combatant == CombatantId(who))
            .unwrap();
        assert_eq!((standing.knockouts, standing.losses), (knockouts, losses));
    }
    let facts = app.world().resource::<KnockoutFacts>();
    assert_eq!(facts.transitions.len(), 2);
    assert_eq!(facts.attributed.len(), 2);
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

/// The game's damage model: a fixture hull at rest, ready for motion on the test's interval.
fn hull_tank(app: &mut App, at: Vec3) -> (Entity, Vec<Entity>) {
    tank(app, at, 0.0, Vec3::ZERO, &hull())
}

/// The game's damage model: collider poses, delivered impacts and whether immutable playback
/// remains at the end of the tick.
struct TickReads {
    poses: Vec<(Vec3, Quat)>,
    impacts: Vec<Vec<(f32, Verdict)>>,
    playing: bool,
}

/// Fire the fixture 88 along `-Z` from `from` and run `ticks` fixed ticks, reading each. A tank's
/// pose is read after the physics interval off its front plate (each of `fronts`, square to the
/// hull at [`hull`]'s front centre): the completed collider pose the game's damage-model walk reads.
fn fire_and_read(app: &mut App, fronts: &[Entity], from: Vec3, ticks: usize) -> Vec<TickReads> {
    app.insert_resource(RetainSpentShells(true));
    let mut seen = 0;
    fire(app, from, FireMechanism::Single);
    let centre = hull()[0].2;
    (0..ticks)
        .map(|_| {
            app.update();
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
            let playing = app
                .world_mut()
                .query::<&Projectile>()
                .iter(app.world())
                .any(|projectile| !projectile.playback.is_empty());
            let mut impacts = vec![Vec::new(); fronts.len()];
            let world = app.world_mut();
            for marks in world.query::<&PenetrationMarks>().iter(world) {
                for (index, mark) in marks.log.iter().enumerate().skip(seen) {
                    let (at, verdict) = match mark {
                        ArmourMark::Crossing { entrance, end, .. } => (
                            *entrance,
                            match end {
                                CrossingEnd::Rest { .. } => Verdict::Stopped,
                                CrossingEnd::Exit { .. } => Verdict::Through,
                            },
                        ),
                        ArmourMark::Bounce { position, .. } => (*position, Verdict::Ricochet),
                        _ => continue,
                    };
                    for (tank, front) in fronts.iter().enumerate() {
                        let owner = world.get::<VolumeOf>(*front).unwrap().tank();
                        let frame = marks
                            .frames
                            .get(&(index, owner))
                            .expect("a runtime contact carries its sampled frame");
                        let (position, rotation) = frame.pose();
                        let z = (rotation.inverse() * (at - position)).z;
                        if z.abs() < 2.0 {
                            impacts[tank].push((z, verdict));
                        }
                    }
                }
                seen = marks.log.len();
            }
            TickReads {
                poses,
                impacts,
                playing,
            }
        })
        .collect()
}

/// The game's damage model: every contact is stated in its own frozen hull frame, even
/// when delivery spans ticks. A tick-end pose substituted for it moves the face off its plane.
fn crossed_whole(reads: &[TickReads], tank: usize) -> usize {
    let faces = hull().map(|(_, size, centre, _)| centre.z + size.z * 0.5);
    let tick = reads
        .iter()
        .position(|read| !read.impacts[tank].is_empty())
        .expect("the fixture tank is crossed");
    let read: Vec<_> = reads
        .iter()
        .flat_map(|read| &read.impacts[tank])
        .copied()
        .collect();
    for &(z, _) in &read {
        assert!(
            faces.iter().any(|face| (z - face).abs() < 1e-3),
            "contact is on its sampled face: {z}, {faces:?}"
        );
    }
    for plate in [faces[0], faces[2]] {
        assert!(
            read.iter()
                .any(|&(z, verdict)| (z - plate).abs() < 1e-3 && verdict.entered()),
            "the plate at {plate} is perforated: {read:?}"
        );
    }
    tick
}

/// The game's damage model: immutable playback reads the moving body's plates at discovery,
/// then delivers them at their real path moments under either collider-placement composition.
#[test]
fn a_moving_tanks_crossing_resolves_against_its_pose_at_contact() {
    for pose in [ChildPose::Avian, ChildPose::Networked] {
        let mut app = recording_world_under(false, pose);
        let (tank, volumes) = hull_tank(&mut app, Vec3::ZERO);
        settle(&mut app);
        app.world_mut()
            .entity_mut(tank)
            .insert(LinearVelocity(Vec3::new(0.0, 0.0, 10.0)));
        let reads = fire_and_read(&mut app, &[volumes[0]], Vec3::new(0.0, 0.0, FROM), 6);
        let contact = crossed_whole(&reads, 0);
        assert!(
            reads[contact].playing,
            "{pose:?}: the crossing continues as immutable playback beyond its contact tick"
        );
        assert!(
            reads[contact + 1].poses[0].0.z - reads[contact].poses[0].0.z > 0.1,
            "{pose:?}: the tank drove on after contact"
        );
        assert!(
            hp(&app, volumes[1]) < DEEP_UNITS,
            "{pose:?}: the crew box took a dose"
        );
    }
}

/// The game's damage model: disjoint bodies have distinct immutable discovery chains. Each
/// plate is read once and each witness takes a dose, despite motion during playback.
#[test]
fn a_round_through_one_tank_into_a_second_resolves_both() {
    for pose in [ChildPose::Avian, ChildPose::Networked] {
        let mut app = recording_world_under(false, pose);
        let (near, near_volumes) = hull_tank(&mut app, Vec3::ZERO);
        let (far, far_volumes) = hull_tank(&mut app, Vec3::new(0.0, 0.0, -9.0));
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
            reads[first].playing,
            "{pose:?}: the near crossing still has playback after its contact tick"
        );
        assert!(
            second > first,
            "{pose:?}: the far tank is discovered after the first crossing"
        );
        for (name, crew) in [("near", near_volumes[1]), ("far", far_volumes[1])] {
            assert!(
                hp(&app, crew) < DEEP_UNITS,
                "{pose:?}: the {name} crew box took a dose"
            );
        }
    }
}

/// A round that leaves a tank and strikes it again off another tank's armour: each encounter is its
/// own record of the one shot, their losses partition the shot's loss on the tank, and a knockout
/// the second encounter dealt is that encounter's. Fixture geometry: the round crosses the struck
/// tank's front box, skids off a thick plate on a second tank 6 m to its side at 65° incidence, and
/// slides along that plate, yawed 10° back toward the struck tank, across the struck tank's rear box
/// and the ammunition behind it, clear of the struck tank's bounds for over a tick in between. The
/// slide stands about 2 m off the tank's axis 45 m behind the front box (DERIVED:
/// 6 m - (45 m - 6 m / tan 15°) × tan 10°).
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
    tank(
        &mut app,
        Vec3::new(6.0, 0.0, -6.0 / 15.0_f32.to_radians().tan()),
        10.0_f32.to_radians(),
        Vec3::ZERO,
        &[(
            "glacis",
            Vec3::new(0.3, 3.0, 40.0),
            Vec3::new(0.15, 0.0, -10.0),
            Kind::Plate,
        )],
    );
    // FIXTURE DATA: the recipient life is the record's live production identity.
    let struck_life = crate::damage_cam::wire::TankLife {
        combatant: crate::CombatantId(90),
        life: crate::CombatantLife(0),
    };
    app.world_mut()
        .entity_mut(struck)
        .insert((struck_life.combatant, struck_life.life));
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
        .filter(|record| record.life == Some(struck_life))
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
    // for 30 at mark 1, each taken into the tally at its own rate: the line's the one energy rate,
    // the cone's its unit.
    let joules = |points: f32| points * super::energy_dose::joules_per_hp() as f32;
    let cone_unit = super::spall_cone::cone_unit();
    let (line_0, line_1) = (joules(40.0), joules(30.0));
    let cone = (20.0 / cone_unit.hp_per_joule()) as f32;
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
        vec![field_units(cone, cone_unit), energy_units(line_1)],
        1,
    );
    assert!(world.get::<ComponentHealth>(ammo).unwrap().is_empty());

    let crossing =
        |entrance: Vec3, volumes: Vec<VolumeCrossing>, exit: Vec3, cones| ArmourMark::Crossing {
            entrance,
            normal: Vec3::Z,
            incoming: -Vec3::Z,
            speed_in: SPEED,
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
        exited: vec![plate],
        origin: z(4.9),
        axis: Dir3::NEG_Z,
        field: super::spall_cone::FieldDescriptor::ball(2.0),
        strength: cone,
        rays: vec![SpallRay {
            direction: Dir3::NEG_Z,
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
        app.insert_resource(crate::ShotClock(crate::sim_tick::SimTick::new(tick)));
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

/// The game's damage model: a walk's push is a next-step input. The physics consumer must
/// integrate it exactly once after the walk; moving the walk before physics moves the body early.
#[test]
fn a_walk_push_changes_position_only_in_the_next_physics_step() {
    let mut app = recording_world(false);
    app.insert_resource(avian3d::prelude::Gravity(Vec3::ZERO));
    // Fixture data: a unit-mass cube away from every other collider.
    let origin = Vec3::splat(50.0);
    let body = app
        .world_mut()
        .spawn((
            Transform::from_translation(origin),
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            Mass(1.0),
            NoAutoMass,
        ))
        .id();
    settle(&mut app);
    app.add_systems(
        FixedPostUpdate,
        (move |mut bodies: Query<Forces>, mut applied: Local<bool>| {
            if !*applied {
                apply_hit_impulse(&mut bodies, body, Vec3::X, origin);
                *applied = true;
            }
        })
        .in_set(SimPhase::ProjectileMarch),
    );
    app.update();
    assert!(
        app.world().get::<LinearVelocity>(body).unwrap().x > 0.0,
        "the walk delivered its impulse"
    );
    assert_eq!(
        app.world().get::<Position>(body).unwrap().0,
        origin,
        "completed physics cannot integrate a later walk's impulse"
    );
    app.update();
    assert!(
        app.world().get::<Position>(body).unwrap().x > origin.x,
        "the next physics step integrates the walk's impulse"
    );
}

/// The game's damage model: a face approaching at fixture 10 m/s must be met in the tick
/// whose tick-start snapshot misses it (DERIVED 1-in-80 snapshot window: 10/800).
/// The authority relies on contact discovery never leaving the next tick inside that face.
#[test]
fn an_approaching_face_is_met_before_the_next_start() {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    let (root, plates) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[(
            "moving plate",
            Vec3::new(4.0, 4.0, 0.05),
            Vec3::ZERO,
            Kind::Plate,
        )],
    );
    settle(&mut app);
    app.world_mut()
        .entity_mut(root)
        .insert(LinearVelocity(Vec3::Z * 10.0));
    let from = Vec3::new(0.0, 0.0, 12.62);
    let snapshot_end = from
        + freeflight_step(
            Vec3::NEG_Z * SPEED,
            fixture_drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
            TICK.as_secs_f32(),
        ) * TICK.as_secs_f32();
    let plate_end = 10.0 * TICK.as_secs_f32();
    assert!(
        (snapshot_end.z - plate_end).abs() < 0.025,
        "the fixture's missed snapshot would leave the next start inside the plate"
    );
    fire(&mut app, from, FireMechanism::Single);
    app.update();
    let mut query = app.world_mut().query::<&PenetrationMarks>();
    let speeds: Vec<_> = query
        .iter(app.world())
        .flat_map(|marks| &marks.log)
        .filter_map(|mark| match mark {
            ArmourMark::Crossing {
                speed_in, volumes, ..
            } if volumes.iter().any(|v| v.volume == plates[0]) => Some(*speed_in),
            ArmourMark::Bounce { speed_in, .. } => Some(*speed_in),
            _ => None,
        })
        .collect();
    assert!(
        !speeds.is_empty(),
        "the moving face contacts this tick despite a snapshot miss"
    );
    assert!(
        speeds[0] > SPEED,
        "approach increases relative impact speed: {}",
        speeds[0]
    );
    for _ in 0..3 {
        app.update();
    }
}

/// The game's damage model: a receding plate takes world velocity minus its point velocity.
/// A world-speed mutant overstates potency; the mechanism and recorded impact rely on this read.
#[test]
fn a_receding_body_takes_a_slower_impact() {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    let (root, _) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[(
            "moving plate",
            Vec3::new(4.0, 4.0, 0.05),
            Vec3::ZERO,
            Kind::Plate,
        )],
    );
    settle(&mut app);
    app.world_mut()
        .entity_mut(root)
        .insert(LinearVelocity(Vec3::NEG_Z * 10.0));
    fire(&mut app, Vec3::new(0.0, 0.0, 5.0), FireMechanism::Single);
    app.update();
    let mut query = app.world_mut().query::<&PenetrationMarks>();
    let speeds: Vec<_> = query
        .iter(app.world())
        .flat_map(|marks| &marks.log)
        .filter_map(|mark| match mark {
            ArmourMark::Crossing { speed_in, .. } | ArmourMark::Bounce { speed_in, .. } => {
                Some(*speed_in)
            }
            _ => None,
        })
        .collect();
    assert!(!speeds.is_empty(), "the receding plate is reached");
    assert!(
        (speeds[0]
            - (freeflight_step(
                Vec3::NEG_Z * SPEED,
                fixture_drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
                TICK.as_secs_f32()
            )
            .length()
                - 10.0))
            .abs()
            < 0.01,
        "relative fixture impact: {}",
        speeds[0]
    );
}

/// The game's damage model: the round's real impulse launches a loose piece faster than the
/// round along its line. The next interval must meet it as a second ordinary contact, rather
/// than letting tick-end geometry cover the next discovery start. Mass is fixture data.
#[test]
fn a_piece_launched_faster_than_the_round_is_met_again() {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true))
        .insert_resource(avian3d::prelude::Gravity(Vec3::ZERO));
    let (piece, _) = tank(
        &mut app,
        Vec3::new(0.0, 20.0, 5.0),
        0.0,
        Vec3::ZERO,
        &[(
            "loose plate",
            Vec3::new(4.0, 4.0, 0.05),
            Vec3::ZERO,
            Kind::Plate,
        )],
    );
    app.world_mut()
        .entity_mut(piece)
        .insert((RigidBody::Dynamic, Mass(0.1)));
    settle(&mut app);
    fire_along(
        &mut app,
        Vec3::Y * 20.0,
        Dir3::Z,
        FireMechanism::Single,
        None,
    );
    app.update();
    let piece_speed = app.world().get::<LinearVelocity>(piece).unwrap().z;
    let mut shells = app.world_mut().query::<(&Projectile, &PenetrationMarks)>();
    let (projectile, marks) = shells.single(app.world()).unwrap();
    assert!(
        piece_speed > projectile.velocity.z,
        "the actual impulse launched the fixture piece faster: {piece_speed} > {}",
        projectile.velocity.z
    );
    let first = marks
        .log
        .iter()
        .filter(|mark| {
            matches!(
                mark,
                ArmourMark::Crossing { .. } | ArmourMark::Bounce { .. }
            )
        })
        .count();
    assert_eq!(first, 1, "the first contact resolved");
    app.update();
    let (_, marks) = shells.single(app.world()).unwrap();
    let contacts = marks
        .log
        .iter()
        .filter(|mark| {
            matches!(
                mark,
                ArmourMark::Crossing { .. } | ArmourMark::Bounce { .. }
            )
        })
        .count();
    assert!(
        contacts > first,
        "the overtaking piece is an ordinary second contact"
    );
}

/// The game's damage model: a third body arriving laterally at first contact belongs to the
/// same frozen union. Its tick-end bounds miss the round, so this also pins broad-phase motion
/// padding. A collector using the live pose would omit its dose. All dimensions are fixture data.
#[test]
fn a_third_body_arriving_at_contact_uses_the_same_sampled_scene() {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[(
            "front plate",
            Vec3::new(4.0, 4.0, 0.05),
            Vec3::ZERO,
            Kind::Plate,
        )],
    );
    let (lateral, third) = tank(
        &mut app,
        Vec3::new(-0.5, 0.0, 0.0),
        0.0,
        Vec3::ZERO,
        &[(
            "arriving plate",
            Vec3::new(0.2, 4.0, 0.05),
            Vec3::ZERO,
            Kind::Crew(DEEP),
        )],
    );
    app.world_mut()
        .entity_mut(third[0])
        .insert(BallisticVolume {
            material: walk::VolumeMaterial::fixture(STEEL * 2.0),
            substance: "fixture".into(),
        });
    settle(&mut app);
    app.world_mut()
        .entity_mut(lateral)
        .insert(LinearVelocity(Vec3::X * 64.0));
    let stepped = freeflight_step(
        Vec3::NEG_Z * SPEED,
        fixture_drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
        TICK.as_secs_f32(),
    );
    let from = Vec3::new(0.0, 0.0, 0.025 - stepped.z * TICK.as_secs_f32() * 0.5);
    fire(&mut app, from, FireMechanism::Single);
    app.update();
    let at_end = app.world().get::<Position>(third[0]).unwrap().x;
    assert!(at_end > 0.4, "the end snapshot misses the round's disc");
    assert!(
        hp(&app, third[0]) < DEEP_UNITS,
        "the arriving body is present in the frozen crossing and takes its dose"
    );
    let mut logs = app.world_mut().query::<&PenetrationMarks>();
    let crossing = logs
        .single(app.world())
        .unwrap()
        .log
        .iter()
        .find_map(|mark| match mark {
            ArmourMark::Crossing { volumes, .. } => Some(volumes),
            _ => None,
        })
        .expect("an ordinary crossing");
    assert!(
        crossing.iter().any(|volume| volume.volume == third[0]),
        "the crossing records the arriving material"
    );
}

/// The game's damage model: a round is placed at its birth, not moved there, so its membership is
/// read at the muzzle. A muzzle inside a foreign body (MEASURED in a 30-bot run: tanks pressed
/// together, one gun's muzzle in the other's turret side) starts the round in that body's
/// material, and the walk carries it out as a crossing of that body; the server relies on this not
/// being read as a broken walk. The same material law handles a body born around a round already
/// in flight. Mutant: omitting birth membership loses the first material chord.
#[test]
fn a_round_born_inside_a_foreign_body_starts_in_its_material() {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    let (_, plates) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[("plate", Vec3::new(2.0, 2.0, 0.5), Vec3::ZERO, Kind::Plate)],
    );
    settle(&mut app);
    fire(&mut app, Vec3::ZERO, FireMechanism::Single);
    run(&mut app, 2);
    let mut query = app.world_mut().query::<&PenetrationMarks>();
    let crossed = query
        .iter(app.world())
        .flat_map(|marks| &marks.log)
        .any(|mark| {
            matches!(mark, ArmourMark::Crossing { volumes, .. }
                if volumes.iter().any(|volume| volume.volume == plates[0]))
        });
    assert!(
        crossed,
        "the round's crossing records the body it was born in"
    );
}

/// The game's damage model: a resolved moving frame can leave zero world speed. The remaining
/// tick still has seconds, and an approaching body can reach the stationary round. Dividing
/// world travel by speed would spend NaN seconds; gameplay relies on the contact fraction instead.
#[test]
fn a_zero_world_speed_waits_or_meets_a_moving_body_on_its_clock() {
    use bevy::ecs::system::RunSystemOnce;
    for enters in [false, true] {
        let mut app = recording_world(false);
        app.insert_resource(RetainSpentShells(true));
        let at = Vec3::new(if enters { 0.0 } else { 10.0 }, 20.0, -1.0);
        let (body, _) = tank(
            &mut app,
            at,
            0.0,
            Vec3::ZERO,
            &[(
                "moving plate",
                Vec3::new(2.0, 2.0, 0.05),
                Vec3::ZERO,
                Kind::Plate,
            )],
        );
        settle(&mut app);
        app.world_mut()
            .entity_mut(body)
            .insert(LinearVelocity(Vec3::Z * 128.0));
        let from = Vec3::Y * 20.0;
        fire_along(&mut app, from, Dir3::Z, FireMechanism::Single, None);
        app.world_mut().flush();
        let playing = app
            .world_mut()
            .run_system_once(
                move |world: ProjectileMarchWorld,
                      colliders: Query<(
                    &'static Position,
                    &'static Rotation,
                    &'static Collider,
                    Option<&'static BallisticSurfaces>,
                )>| {
                    let context = resolve::ResolveContext {
                        world: &world,
                        colliders: &colliders,
                        deposit: true,
                        laws: walk::WalkLaws::default(),
                    };
                    let frame = walk::DiscFrame::anchored(Vec3::Z).unwrap();
                    let interior = resolve::Interior::laid(
                        from,
                        resolve::layout(&frame, resolve::disc_radius(FIXTURE_88.caliber)),
                    );
                    let flight = resolve::Flight {
                        position: from,
                        direction: Dir3::Z,
                        speed: SPEED,
                        frame,
                        continuation: Some(interior.clone()),
                        membership: resolve::Membership::Established,
                    };
                    // Fixture: the completed relative path's carrier exactly cancels its world velocity.
                    let mut playing = resolve::Playback::new(
                        resolve::Decision::Clear {
                            resume: from,
                            travel: 0.0,
                            continuation: interior,
                            leg: resolve::Leg::new(from, Dir3::Z, 0.0, &[Vec3::ZERO]),
                        },
                        &flight,
                        bound(FIXTURE_88),
                        FlyingBody {
                            mass: FIXTURE_88.mass,
                            caliber: FIXTURE_88.caliber,
                            drag_k: 0.0,
                        },
                        &context,
                        &|_| true,
                        0.0,
                        &|_| false,
                    );
                    playing.in_moving_frame(Vec3::NEG_Z * SPEED);
                    assert_eq!(playing.flight().speed, 0.0, "the fixture leaves world rest");
                    playing
                },
            )
            .unwrap();
        let mut shells = app.world_mut().query::<&mut Projectile>();
        shells
            .single_mut(app.world_mut())
            .unwrap()
            .playback
            .push_back(playing);
        app.update();
        let mut logs = app.world_mut().query::<(&Transform, &PenetrationMarks)>();
        let (pose, marks) = logs.single(app.world()).unwrap();
        if enters {
            assert!(
                marks.log.iter().any(|mark| matches!(mark,
                ArmourMark::Crossing { speed_in, .. } if (*speed_in - 128.0).abs() < 0.01)),
                "the moving body supplies the relative impact"
            );
        } else {
            assert!(
                marks.log.is_empty(),
                "no static contact was invented at rest"
            );
            assert_eq!(
                pose.translation, from,
                "open world rest spends seconds without movement"
            );
        }
    }
}

/// The game's damage model: discovery's fuze keeps its seconds when a crossing slows the round,
/// as playback's does ([`follow_fuzed`]). The consumer is a delayed `Impact` burst whose point
/// falls in the gap between two parts of one body: discovery freezes that gap in the same
/// immutable chain as the slab before it, so a count left at the entry speed carries the burst
/// past its point (DERIVED: by the factor `v_in / v_out` on the metres left) and onto the rear
/// part, with the whole round's crossing decided there. Contact falls late in a tick, so the
/// chain's playback spans the boundary. Mutant: dropping the rescale in discovery carries the
/// burst onto the rear part (MEASURED: to its face). All dimensions, speeds and the delay are
/// fixture data.
#[test]
fn a_slowed_impact_delay_bursts_where_its_seconds_run_out() {
    // Fixture: a 120 mm fixture-steel slab the 88 perforates and leaves much slower.
    const SLAB: f32 = 0.12;
    let slab = ("slab", Vec3::new(3.0, 3.0, SLAB), Vec3::ZERO, Kind::Plate);
    // Contact about 0.92 of a tick after the shot (12.5 m per tick at the fixture speed).
    let from = Vec3::new(0.0, 0.0, 0.5 * SLAB + 11.5);
    let marks_of = |app: &mut App| -> Vec<ArmourMark> {
        let mut query = app.world_mut().query::<&PenetrationMarks>();
        query
            .iter(app.world())
            .flat_map(|marks| marks.log.clone())
            .collect()
    };

    // Calibration: the slab's entry and exit speeds under the fixture round, inert.
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    tank(&mut app, Vec3::ZERO, 0.0, Vec3::ZERO, &[slab]);
    settle(&mut app);
    fire(&mut app, from, FireMechanism::Single);
    run(&mut app, 4);
    let (v_in, v_out) = marks_of(&mut app)
        .iter()
        .find_map(|mark| match mark {
            ArmourMark::Crossing {
                speed_in,
                end: CrossingEnd::Exit { speed, .. },
                ..
            } => Some((*speed_in, *speed)),
            _ => None,
        })
        .expect("fixture: the 88 perforates the slab");
    let r = v_out / v_in;
    assert!(
        r < 0.8,
        "fixture: the slab slows the round hard ({v_in} -> {v_out} m/s)"
    );

    // The delay leaves `left` metres of path past the slab at the entry speed's count: the
    // correct burst lies `left * r` past the slab, the stale count's `left` past it. The rear
    // part's face sits halfway between.
    let left = 4.0;
    let delay = (left + SLAB) / v_in;
    let gap = left * (1.0 + r) / 2.0;
    let back = -0.5 * SLAB;
    let rear = (
        "rear",
        Vec3::new(3.0, 3.0, THIN),
        Vec3::new(0.0, 0.0, back - gap - 0.5 * THIN),
        Kind::Plate,
    );
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    tank(&mut app, Vec3::ZERO, 0.0, Vec3::ZERO, &[slab, rear]);
    settle(&mut app);
    let inputs = burst::BurstInputs {
        trigger: crate::rounds::fuze::Trigger::Impact {
            sensitivity_n: 30.0,
        },
        delay,
        ..burst::fixture_inputs()
    };
    let burst = burst::Burst::derived(FIXTURE_88, burst::FIXTURE_STEEL, &inputs).unwrap();
    fire_round(
        &mut app,
        from,
        Dir3::NEG_Z,
        FireMechanism::Single,
        None,
        Some(burst),
    );
    let burst_at = |marks: &[ArmourMark]| {
        marks.iter().find_map(|mark| match mark {
            ArmourMark::Burst { at, .. } => Some(*at),
            _ => None,
        })
    };
    run(&mut app, 1);
    let first = marks_of(&mut app);
    assert!(
        first
            .iter()
            .any(|mark| matches!(mark, ArmourMark::Crossing { .. })),
        "the round meets the slab in the first tick: {first:?}"
    );
    assert!(
        burst_at(&first).is_none(),
        "the chain's playback reaches the burst only in a later tick"
    );
    run(&mut app, 3);
    let at = burst_at(&marks_of(&mut app)).expect("the round bursts");
    let past = back - at.z;
    assert!(
        past > 0.0 && past < gap,
        "the burst lies in the gap ({gap} m), {past} m past the slab"
    );
    assert!(
        (past - left * r).abs() < 0.05,
        "the delay's seconds reach {} m past the slab at the exit speed, not {past} m",
        left * r
    );
}

/// The game's damage model: production discovery publishes complete, serializable paths before
/// playback spans later ticks. Replicas can replay those facts without the target's geometry.
#[test]
fn production_discovery_exports_a_valid_clock_before_crossing_playback_finishes() {
    #[derive(Resource, Default)]
    struct Paths(Vec<FlightOutcome>);
    fn capture(outcome: On<FlightOutcome>, mut paths: ResMut<Paths>) {
        paths.0.push(outcome.clone());
    }
    let mut app = recording_world(false);
    app.insert_resource(crate::ShotClock(crate::sim_tick::SimTick::new(100)))
        .init_resource::<Paths>()
        .add_observer(capture);
    tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[("slab", Vec3::new(3.0, 3.0, 0.12), Vec3::ZERO, Kind::Plate)],
    );
    settle(&mut app);
    let shot = crate::ShotId {
        combatant: crate::CombatantId(7),
        weapon: 0,
        fire_tick: 100,
    };
    let stepped = freeflight_step(
        Vec3::NEG_Z * SPEED,
        fixture_drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
        TICK.as_secs_f32(),
    );
    // DERIVED fixture placement: first contact is one millimetre short of this tick's endpoint.
    fire_along(
        &mut app,
        Vec3::new(0.0, 0.0, 0.06 - stepped.z * TICK.as_secs_f32() - 0.001),
        Dir3::NEG_Z,
        FireMechanism::Single,
        Some(shot),
    );
    run(&mut app, 1);
    let paths = &app.world().resource::<Paths>().0;
    assert!(
        !paths.is_empty(),
        "discovery publishes when the first contact is reached"
    );
    let path = paths
        .iter()
        .find(|path| !path.spans.is_empty())
        .expect("material playback has a path");
    assert_eq!(path.validate(), Ok(()));
    assert!(
        f64::from(path.tick - 100) + path.fraction + path.seconds() / TICK.as_secs_f64() > 1.0,
        "the declared clock includes material that will play in a later tick"
    );
    let bytes = postcard::to_allocvec(path).unwrap();
    let decoded: FlightOutcome = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.validate(), Ok(()));
    assert_eq!(decoded.seconds(), path.seconds());
    assert_eq!(decoded.resume, path.resume);
}

/// The game's damage model: a collider born around an in-flight round owns material at its
/// birth boundary. Respawn geometry can create this state without any missed surface; the
/// authority must resolve its remaining chord. All dimensions and speeds are fixture data.
#[test]
fn a_body_born_around_an_in_flight_round_resolves_its_material() {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    settle(&mut app);
    fire(&mut app, Vec3::new(0.0, 20.0, 30.0), FireMechanism::Single);
    app.update();
    let mut shells = app
        .world_mut()
        .query_filtered::<&Transform, With<Projectile>>();
    let at = shells.single(app.world()).unwrap().translation;
    let (_, plates) = tank(
        &mut app,
        at,
        0.0,
        Vec3::ZERO,
        &[(
            "new solid",
            Vec3::new(2.0, 2.0, 0.1),
            Vec3::ZERO,
            Kind::Plate,
        )],
    );
    run(&mut app, 3);
    let mut logs = app.world_mut().query::<&PenetrationMarks>();
    assert!(
        logs.iter(app.world())
            .flat_map(|marks| &marks.log)
            .any(|mark| matches!(mark,
        ArmourMark::Crossing { volumes, .. } if volumes.iter().any(|v| v.volume == plates[0]))),
        "the new body's material is crossed, never skipped"
    );
}

/// The game's damage model: a moving body born around an in-flight round is crossed in its own
/// frame. The contained contact takes the body's point velocity as the carrier, so the crossing's
/// entry speed and heading are the round's relative ones. Mutant: a contained contact with zero
/// velocity resolves the world heading (`incoming.x` zero). Dimensions and speeds are fixture data.
#[test]
fn a_moving_body_born_around_an_in_flight_round_crosses_it_in_its_frame() {
    const BODY: Vec3 = Vec3::new(256.0, 0.0, 0.0);
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    settle(&mut app);
    fire(&mut app, Vec3::new(0.0, 20.0, 30.0), FireMechanism::Single);
    app.update();
    let mut shells = app
        .world_mut()
        .query_filtered::<(&Transform, &Projectile), With<Projectile>>();
    let (pose, projectile) = shells.single(app.world()).unwrap();
    let (at, velocity) = (pose.translation, projectile.velocity);
    let (_, plates) = tank(
        &mut app,
        at,
        0.0,
        BODY,
        &[("moving", Vec3::new(4.0, 2.0, 0.1), Vec3::ZERO, Kind::Plate)],
    );
    run(&mut app, 3);
    let mut logs = app.world_mut().query::<&PenetrationMarks>();
    let (incoming, speed_in) = logs
        .iter(app.world())
        .flat_map(|marks| &marks.log)
        .find_map(|mark| match mark {
            ArmourMark::Crossing {
                volumes,
                incoming,
                speed_in,
                ..
            } if volumes.iter().any(|v| v.volume == plates[0]) => Some((*incoming, *speed_in)),
            _ => None,
        })
        .expect("the moving body's material is crossed");
    let relative = velocity - BODY;
    assert!(
        incoming.x < -0.1,
        "the crossing heading is relative to the body: {incoming:?}"
    );
    assert!(
        (speed_in - relative.length()).abs() < 0.05 * relative.length(),
        "the crossing enters at the relative speed: {speed_in} vs {}",
        relative.length()
    );
}

/// The game's damage model: material born around a round at relative rest holds it with no
/// relative motion, so no crossing clock can be built from it (travel over zero speed). The
/// authority must neither panic nor skip the material: the round rests in the material's frame
/// to the tick boundary and its next step reads that membership again. Both a resisting and a
/// load-admitted zero-resistance plate; the zeroed velocity and plates are fixture data.
#[test]
fn a_round_at_relative_rest_inside_new_material_builds_no_zero_clock() {
    for factor in [0.0, STEEL] {
        let mut app = recording_world(false);
        app.insert_resource(RetainSpentShells(true));
        settle(&mut app);
        fire(&mut app, Vec3::new(0.0, 20.0, 30.0), FireMechanism::Single);
        app.update();
        let mut shells = app
            .world_mut()
            .query_filtered::<(&Transform, &mut Projectile), With<Projectile>>();
        let (pose, mut projectile) = shells.single_mut(app.world_mut()).unwrap();
        let at = pose.translation;
        // Fixture: the next free-flight step integrates exactly zero world velocity.
        projectile.velocity = -gravity() * TICK.as_secs_f32();
        let root = app
            .world_mut()
            .spawn((Transform::from_translation(at), RigidBody::Static))
            .id();
        let plate = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                ChildOf(root),
                Collider::cuboid(2.0, 2.0, 0.1),
                CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                BallisticVolume {
                    material: walk::VolumeMaterial::fixture(factor),
                    substance: "fixture".to_owned(),
                },
                VolumeOf(root),
            ))
            .id();
        run(&mut app, 4);
        if factor == 0.0 {
            continue; // Unresisting material admits the round's flight; the authority ran on.
        }
        let mut logs = app.world_mut().query::<&PenetrationMarks>();
        assert!(
            logs.iter(app.world())
                .flat_map(|marks| &marks.log)
                .any(|mark| matches!(mark,
                ArmourMark::Crossing { volumes, .. } if volumes.iter().any(|v| v.volume == plate))),
            "the held material is crossed once relative motion resumes"
        );
    }
}

/// The game's damage model: two overlapping bodies moving together share one crossing, whose
/// world path curves as resistance slows the relative round. The authority's record admission
/// must accept each body's departure. Dimensions and transverse velocity are fixture data.
#[test]
fn a_slowing_shared_crossing_in_a_moving_frame_is_admitted() {
    let mut app = recording_world(false);
    for (name, thickness, z) in [("front", 0.05, 0.0), ("behind", 0.5, -0.24)] {
        tank(
            &mut app,
            Vec3::new(0.0, 20.0, 0.0),
            0.0,
            Vec3::X * 64.0,
            &[(
                name,
                Vec3::new(4.0, 4.0, thickness),
                Vec3::Z * z,
                Kind::Plate,
            )],
        );
    }
    settle(&mut app);
    // Follow the bodies' transverse location after the fixture registration steps.
    let mut roots = app.world_mut().query_filtered::<&Position, With<Rig>>();
    let x = roots.iter(app.world()).next().unwrap().x;
    fire(&mut app, Vec3::new(x, 20.0, 2.0), FireMechanism::Single);
    let records = run(&mut app, 5);
    assert_eq!(records.len(), 2, "each overlapping body records");
    for recorded in records {
        let mut report = recorded.report;
        report.round = "88mm_AP".to_owned();
        crate::damage_cam::tests::assert_self_consistent("moving shared crossing", &report);
        let text = ron::ser::to_string(&report).unwrap();
        crate::damage_cam::PenetrationReport::from_ron(&text)
            .expect("a slowing moving crossing is a legitimate record");
    }
}

/// The game's damage model: resting relative to one material cannot hide a different body's
/// plate later in the same tick; collision discovery still owns the remaining world path.
/// All dimensions and velocities below are fixture data.
#[test]
fn relative_rest_still_discovers_a_separate_plate_in_the_same_tick() {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    settle(&mut app);
    fire(&mut app, Vec3::new(0.0, 20.0, 30.0), FireMechanism::Single);
    app.update();
    let mut shells = app.world_mut().query::<(&mut Transform, &mut Projectile)>();
    let (mut pose, mut projectile) = shells.single_mut(app.world_mut()).unwrap();
    let at = Vec3::new(0.0, 20.0, 32.0);
    pose.translation = at;
    projectile.starts = Some(resolve::Interior::laid(
        at,
        resolve::layout(
            &projectile.disc,
            resolve::disc_radius(projectile.flying.caliber),
        ),
    ));
    let velocity = Vec3::NEG_Z * 640.0;
    projectile.flying.drag_k = 0.0;
    projectile.velocity = velocity - gravity() * TICK.as_secs_f32();
    let (_, carrier) = tank(
        &mut app,
        at,
        0.0,
        velocity,
        &[("carrier", Vec3::splat(2.0), Vec3::ZERO, Kind::Plate)],
    );
    let (_, separate) = tank(
        &mut app,
        at + Vec3::NEG_Z * 5.0,
        0.0,
        Vec3::ZERO,
        &[(
            "separate",
            Vec3::new(4.0, 4.0, 0.5),
            Vec3::ZERO,
            Kind::Plate,
        )],
    );
    // The containing material has no relative traversal, regardless of its resistance.
    assert_ne!(carrier[0], separate[0]);
    app.update();
    let mut logs = app.world_mut().query::<&PenetrationMarks>();
    let marks: Vec<_> = logs
        .iter(app.world())
        .flat_map(|marks| &marks.log)
        .collect();
    assert!(
        marks.iter().any(|mark| matches!(mark,
        ArmourMark::Crossing { volumes, .. } if volumes.iter().any(|v| v.volume == separate[0]))),
        "the separate plate must resolve before this tick ends: {marks:?}"
    );
}

/// The game's damage model: a barrel through foreign armour or a wall cannot skip it. The
/// ordinary path starts outside the source physics hull and must pay the intervening plate.
/// Every dimension, pose and round below is fixture data.
fn barrel_through_plate(owned: bool) {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    let source_at = Vec3::new(0.0, 20.0, 0.0);
    let shape = Collider::cuboid(2.0, 2.0, 2.0);
    let source = app
        .world_mut()
        .spawn((
            Transform::from_translation(source_at),
            RigidBody::Static,
            shape.clone(),
            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
        ))
        .id();
    let at = source_at + Vec3::NEG_Z * 3.0;
    let plate = if owned {
        tank(
            &mut app,
            at,
            0.0,
            Vec3::ZERO,
            &[(
                "foreign plate",
                Vec3::new(4.0, 4.0, 0.5),
                Vec3::ZERO,
                Kind::Plate,
            )],
        )
        .1[0]
    } else {
        app.world_mut()
            .spawn((
                Transform::from_translation(at),
                RigidBody::Static,
                Collider::cuboid(4.0, 4.0, 0.5),
                CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                BallisticVolume {
                    material: walk::VolumeMaterial::fixture(STEEL),
                    substance: "fixture".into(),
                },
            ))
            .id()
    };
    settle(&mut app);
    let muzzle = source_at + Vec3::NEG_Z * 6.0;
    let round = crate::tank::WeaponRound::resolve("88mm_AP").unwrap().round;
    // The weapon pivots at the source's centre, six metres back along the bore.
    let birth = crate::shooting::outside_source(
        muzzle,
        Dir3::NEG_Z,
        -6.0,
        &[(source_at, Quat::IDENTITY, &shape)],
    );
    let fire = FireShell::of_round(
        &round,
        birth,
        muzzle,
        Dir3::NEG_Z,
        FireMechanism::Single,
        true,
        Some(ShotSource {
            tank: source,
            weapon: 0,
            round: 0,
        }),
        None,
    );
    app.world_mut().trigger(fire);
    run(&mut app, 4);
    let mut logs = app.world_mut().query::<&PenetrationMarks>();
    let crossed = logs
        .iter(app.world())
        .flat_map(|marks| &marks.log)
        .find_map(|mark| match mark {
            ArmourMark::Crossing {
                entrance, volumes, ..
            } => volumes
                .iter()
                .find(|v| v.volume == plate)
                .map(|volume| (*entrance, *volume)),
            _ => None,
        })
        .expect("a barrel poked through a plate still fires into that plate from outside");
    assert!(crossed.0.z > at.z, "the plate is met at its entering face");
    assert!(
        crossed.1.cost > 0.0 && !crossed.1.perforated,
        "the intervening plate must be defeated; this thick fixture plate stops the round"
    );
}

/// The game's damage model: a foreign hull's plate remains between the source exit and muzzle.
#[test]
fn a_barrel_through_foreign_armour_still_meets_the_plate() {
    barrel_through_plate(true);
}

/// The game's damage model: a wall between the source exit and muzzle remains ordinary material.
#[test]
fn a_barrel_through_a_wall_still_meets_the_plate() {
    barrel_through_plate(false);
}

/// The game's damage-model presentation: the replay includes the barrel-length prefix while the
/// fire fact keeps the visual muzzle. Fixture data: five metres of bore path, a traced round and
/// cursor fractions chosen on either side of the muzzle within the first tick.
#[test]
fn the_presented_march_includes_the_birth_to_muzzle_prefix() {
    let mut app = recording_world(true);
    settle(&mut app);
    let birth = Vec3::new(0.0, 20.0, 0.0);
    let muzzle = birth + Vec3::NEG_Z * 5.0;
    let round = crate::tank::WeaponRound::resolve("88mm_AP").unwrap().round;
    let mut fire = FireShell::of_round(
        &round,
        birth,
        muzzle,
        Dir3::NEG_Z,
        FireMechanism::Single,
        true,
        None,
        None,
    );
    fire.shot_origin = FireShellOrigin::Reconstructed;
    app.world_mut().trigger(fire);
    app.world_mut().resource_mut::<PathClock>().fraction = 0.25;
    app.update();
    let mut shells = app.world_mut().query::<(&Transform, &ShellPath)>();
    let (pose, path) = shells.single(app.world()).unwrap();
    assert_eq!(
        path.points[0], birth,
        "replay starts at authoritative birth"
    );
    assert!(
        pose.translation.z < birth.z && pose.translation.z > muzzle.z,
        "the first fractional presentation lies inside the barrel-length prefix"
    );
    app.world_mut().resource_mut::<PathClock>().fraction = 0.75;
    app.update();
    let mut shells = app.world_mut().query::<&Transform>();
    let pose = shells.single(app.world()).unwrap();
    assert!(
        pose.translation.z < muzzle.z,
        "the round passes the muzzle inside its first tick"
    );
}

/// The game's damage model: leaving the source's box ends its launch clearance permanently.
/// A real armour skid can then bring the round into its source's rear volumes and deposit dose:
/// the reflector, yawed 10° back toward the source, carries the slide into them. All poses, sizes,
/// pools, speed and angles below are fixture data. Faces are fixed: this tests source admission,
/// independently of carrier motion placing the disc too deep to turn forward.
#[test]
fn a_round_that_bounces_back_into_its_source_strikes_it() {
    returning_source_round(false);
}

/// The game's damage model: a foreign plate inside the source box opens a queued chain whose
/// later reflector is outside it. The authority must charge source armour on the return.
#[test]
fn queued_decisions_meet_source_armour_after_leaving_its_box() {
    returning_source_round(true);
}

fn returning_source_round(queued: bool) {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    let (source, volumes) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[
            (
                "source front",
                Vec3::new(2.0, 2.0, 0.4),
                Vec3::ZERO,
                Kind::Crew(DEEP),
            ),
            (
                "source rear",
                Vec3::new(6.0, 2.0, 0.4),
                Vec3::NEG_Z * 45.0,
                Kind::Crew(DEEP),
            ),
            (
                "source ammo",
                Vec3::new(6.0, 2.0, 0.4),
                Vec3::NEG_Z * 46.0,
                Kind::Ammo(ONE),
            ),
            (
                "source armour",
                Vec3::new(6.0, 2.0, 0.001),
                Vec3::NEG_Z * 44.0,
                Kind::Plate,
            ),
        ],
    );
    tank(
        &mut app,
        Vec3::new(6.0, 0.0, -6.0 / 15.0_f32.to_radians().tan()),
        10.0_f32.to_radians(),
        Vec3::ZERO,
        &[(
            "reflector",
            Vec3::new(0.3, 3.0, 40.0),
            Vec3::new(0.15, 0.0, -10.0),
            Kind::Plate,
        )],
    );
    if queued {
        tank(
            &mut app,
            Vec3::ZERO,
            0.0,
            Vec3::ZERO,
            &[
                (
                    "first contact inside source box",
                    Vec3::new(2.0, 3.0, 0.01),
                    Vec3::new(2.0, 0.0, -7.5),
                    Kind::Plate,
                ),
                (
                    "foreign plate beyond returning source contact",
                    Vec3::new(8.0, 3.0, 0.001),
                    Vec3::NEG_Z * 47.0,
                    Kind::Plate,
                ),
            ],
        );
    }
    settle(&mut app);
    let angle = 15.0_f32.to_radians();
    let direction = Dir3::new(Vec3::new(angle.sin(), 0.0, -angle.cos())).unwrap();
    let round = crate::tank::WeaponRound::resolve("88mm_AP").unwrap().round;
    let mut fire = FireShell::of_round(
        &round,
        Vec3::ZERO,
        Vec3::ZERO,
        direction,
        FireMechanism::Single,
        false,
        Some(ShotSource {
            tank: source,
            weapon: 0,
            round: 0,
        }),
        None,
    );
    fire.speed = SPEED;
    app.world_mut().trigger(fire);
    run(&mut app, 16);
    let mut logs = app.world_mut().query::<&PenetrationMarks>();
    let marks: Vec<_> = logs
        .iter(app.world())
        .flat_map(|marks| &marks.log)
        .collect();
    assert!(
        marks
            .iter()
            .any(|mark| matches!(mark, ArmourMark::Bounce { .. })),
        "the fixture must bounce"
    );
    assert!(
        marks.iter().any(
            |mark| matches!(mark, ArmourMark::Wound { volumes: crossed, .. } | ArmourMark::Crossing { volumes: crossed, .. }
        if crossed.iter().any(|v| v.volume == volumes[1]))
        ),
        "after box clearance the sliding round must strike its source's rear volume: {marks:?}"
    );
    assert!(
        marks.iter().any(|mark| matches!(mark,
            ArmourMark::Crossing { volumes: crossed, .. }
            if crossed.iter().any(|v| v.volume == volumes[3] && v.cost > 0.0))),
        "the returning path must resolve source armour: {marks:?}"
    );
    let mut doses = app.world_mut().query::<&ShotDose>();
    assert!(
        doses.iter(app.world()).any(|dose| dose
            .shown_losses()
            .find(|(volume, _)| *volume == volumes[1])
            .map_or(0, |(_, loss)| loss)
            > 0),
        "the source receives the returning round's dose"
    );
}

/// The game's damage model: translation of the source cannot clear a round that remains inside
/// its body-local box. Fixture data deliberately moves the carrier farther than its whole box in
/// one tick, so a box in the tick-end world frame would clear it early and resolve its own plate.
#[test]
fn source_clearance_travels_with_the_firing_body() {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    let (source, _) = tank(
        &mut app,
        Vec3::new(0.0, 20.0, 0.0),
        0.0,
        Vec3::X * 1000.0,
        &[("source material", Vec3::splat(2.0), Vec3::ZERO, Kind::Plate)],
    );
    settle(&mut app);
    let at = app.world().get::<Position>(source).unwrap().0;
    let round = crate::tank::WeaponRound::resolve("88mm_AP").unwrap().round;
    let mut fire = FireShell::of_round(
        &round,
        at,
        at,
        Dir3::X,
        FireMechanism::Single,
        false,
        Some(ShotSource {
            tank: source,
            weapon: 0,
            round: 0,
        }),
        None,
    );
    fire.speed = 1000.0;
    fire.drag_k = 0.0;
    app.world_mut().trigger(fire);
    run(&mut app, 1);
    let mut shells = app.world_mut().query::<(&Projectile, &PenetrationMarks)>();
    let (round, marks) = shells.single(app.world()).unwrap();
    assert!(
        !round.source_cleared,
        "the round remains inside its moving source box"
    );
    assert!(
        marks.log.is_empty(),
        "initial source clearance cannot charge its own material"
    );
}

/// The game's damage model: launch clearance ends only once the whole disc has left the source
/// box. A foreign plate at 30° inside the box, hull-to-hull, staggers the perforating disc by up to
/// r·tan 30° ≈ 25 mm (DERIVED, r = 44 mm) along the travel, so when the axis clears the box face
/// at the muzzle its lagging samples still sit inside the barrel volume that reaches that face. A
/// source made solid there charges its own barrel with the round's line, cost and dose. The
/// foreign plate's spall cone may still dose the barrel: a field is no launch path. Every pose,
/// size, pool and speed below is fixture data.
#[test]
fn a_staggered_disc_clears_its_whole_source_box_before_the_source_turns_solid() {
    lagging_samples_clear_the_barrel(Vec3::ZERO, false);
}

/// [`a_staggered_disc_clears_its_whole_source_box_before_the_source_turns_solid`] with the foreign
/// body moving (fixture velocity), so the hand-off re-certifies contact-time membership.
#[test]
fn a_staggered_disc_clears_its_source_box_in_a_moving_scene() {
    lagging_samples_clear_the_barrel(Vec3::X * 0.5, false);
}

/// The game's damage model: a tick ending during a perforation retains the admission of its
/// certified sample paths. A planar disc at the playback cursor cannot clear lagging samples.
#[test]
fn partial_playback_keeps_launch_clearance_until_its_samples_resume() {
    lagging_samples_clear_the_barrel(Vec3::ZERO, true);
}

fn lagging_samples_clear_the_barrel(foreign_velocity: Vec3, across_tick: bool) {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    // The barrel alone spans the source box: its front face at z = -3 is the box's.
    let (source, volumes) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[(
            "barrel",
            Vec3::new(0.2, 0.2, if across_tick { 15.0 } else { 3.0 }),
            Vec3::new(0.0, 0.0, if across_tick { 4.5 } else { -1.5 }),
            Kind::Crew(DEEP),
        )],
    );
    tank(
        &mut app,
        Vec3::new(0.0, 0.0, if across_tick { -3.0 } else { -2.9 }),
        30.0_f32.to_radians(),
        foreign_velocity,
        &[(
            "oblique plate",
            Vec3::new(1.0, 1.0, 0.03),
            Vec3::ZERO,
            Kind::Plate,
        )],
    );
    settle(&mut app);
    let round = crate::tank::WeaponRound::resolve("88mm_AP").unwrap().round;
    let birth = Vec3::new(0.0, 0.0, if across_tick { 9.493 } else { -0.5 });
    let mut fire = FireShell::of_round(
        &round,
        birth,
        birth,
        Dir3::NEG_Z,
        FireMechanism::Single,
        false,
        Some(ShotSource {
            tank: source,
            weapon: 0,
            round: 0,
        }),
        None,
    );
    fire.speed = SPEED;
    fire.drag_k = 0.0;
    app.world_mut().trigger(fire);
    if across_tick {
        run(&mut app, 1);
        let mut shells = app.world_mut().query::<(&Projectile, &Transform)>();
        let (projectile, pose) = shells.single(app.world()).unwrap();
        assert!(
            !projectile.playback.is_empty(),
            "the fixture tick ends in playback"
        );
        assert!(
            pose.translation.z < -3.0,
            "the fixture axis has passed the source face: {:?}",
            pose.translation
        );
        assert!(
            !projectile.source_cleared,
            "partial playback cannot clear certified samples from a planar cursor"
        );
    }
    run(&mut app, 4);
    let mut logs = app.world_mut().query::<&PenetrationMarks>();
    let marks: Vec<_> = logs
        .iter(app.world())
        .flat_map(|marks| &marks.log)
        .collect();
    assert!(
        marks
            .iter()
            .any(|mark| matches!(mark, ArmourMark::Crossing { .. })),
        "the fixture perforates the foreign plate: {marks:?}"
    );
    assert!(
        !marks.iter().any(|mark| matches!(mark,
            ArmourMark::Wound { volumes: crossed, .. } | ArmourMark::Crossing { volumes: crossed, .. }
            if crossed.iter().any(|v| v.volume == volumes[0]))),
        "launch clearance never charges the source's own barrel: {marks:?}"
    );
}

/// The game's damage model: an exit followed by an immediate return still ends launch clearance.
/// An axis-only round leaves its source box at x = 1, meets a foreign reflector 0.1 mm past that
/// face at 75° obliquity and slides along it. Resting a body radius off that face, its axis stands
/// back inside the source's box (DERIVED), after launch clearance has ended. The slide must meet
/// the source's armour solid. Every pose, size and speed below is fixture data.
#[test]
fn a_bounce_just_past_the_source_box_returns_into_a_solid_source() {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    let (source, volumes) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[
            (
                "source front",
                Vec3::new(2.0, 2.0, 0.4),
                Vec3::ZERO,
                Kind::Plate,
            ),
            (
                "source armour",
                Vec3::new(2.0, 2.0, 0.01),
                Vec3::NEG_Z * 5.0,
                Kind::Plate,
            ),
        ],
    );
    let angle = 15.0_f32.to_radians();
    let birth = Vec3::new(0.5, 0.0, 0.0);
    // Where the axis meets x = 1.0001: (1.0001 - 0.5) / sin 15° of travel.
    let strike = birth.z - (1.0001 - birth.x) / angle.tan();
    tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[(
            "reflector",
            Vec3::new(0.3, 3.0, 3.0),
            Vec3::new(1.0001 + 0.15, 0.0, strike),
            Kind::Plate,
        )],
    );
    settle(&mut app);
    let round = crate::tank::WeaponRound::resolve("7.9mm_AP").unwrap().round;
    assert_eq!(
        resolve::disc_radius(round.flying.caliber),
        0.0,
        "the fixture round samples on its axis alone"
    );
    let direction = Dir3::new(Vec3::new(angle.sin(), 0.0, -angle.cos())).unwrap();
    let mut fire = FireShell::of_round(
        &round,
        birth,
        birth,
        direction,
        FireMechanism::Single,
        false,
        Some(ShotSource {
            tank: source,
            weapon: 0,
            round: 0,
        }),
        None,
    );
    fire.speed = SPEED;
    app.world_mut().trigger(fire);
    run(&mut app, 4);
    let mut logs = app.world_mut().query::<&PenetrationMarks>();
    let marks: Vec<_> = logs
        .iter(app.world())
        .flat_map(|marks| &marks.log)
        .collect();
    assert!(
        marks
            .iter()
            .any(|mark| matches!(mark, ArmourMark::Bounce { .. })),
        "the fixture must bounce: {marks:?}"
    );
    assert!(
        marks.iter().any(|mark| matches!(mark,
            ArmourMark::Crossing { volumes: crossed, .. }
            if crossed.iter().any(|v| v.volume == volumes[1] && v.cost > 0.0))),
        "the return must resolve the source's armour: {marks:?}"
    );
}

/// The game's damage model: a round with nothing left to move on comes to rest, so a spent round
/// trapped in a closed box ends. The live record relies on it: `finish_records` closes only on a
/// round at rest or clear of the tank, and each further skid would write marks, strikes and impulses
/// without bound. The last mark says the face held it, with no entry, and the record says it rests
/// on that face. Its first reached face is oblique within the Held friction cone, so an ordinary
/// skid followed by a solver stop cannot imitate Held. The box tilts: an exactly head-on read
/// bites. Dimensions, tilt, speed and direction are fixture data. Mutant: no [`walk::Begin::Held`].
#[test]
fn a_spent_round_trapped_in_a_closed_box_comes_to_rest() {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    let (hull, walls) = tank(
        &mut app,
        Vec3::Y * 20.0,
        0.0,
        Vec3::ZERO,
        &[
            (
                "right",
                Vec3::new(0.4, 4.0, 4.0),
                Vec3::X * 1.6,
                Kind::Plate,
            ),
            (
                "left",
                Vec3::new(0.4, 4.0, 4.0),
                Vec3::NEG_X * 1.6,
                Kind::Plate,
            ),
            (
                "front",
                Vec3::new(4.0, 4.0, 0.4),
                Vec3::Z * 1.6,
                Kind::Plate,
            ),
            (
                "back",
                Vec3::new(4.0, 4.0, 0.4),
                Vec3::NEG_Z * 1.6,
                Kind::Plate,
            ),
            ("roof", Vec3::new(4.0, 0.4, 4.0), Vec3::Y * 1.6, Kind::Plate),
            (
                "floor",
                Vec3::new(4.0, 0.4, 4.0),
                Vec3::NEG_Y * 1.6,
                Kind::Plate,
            ),
        ],
    );
    app.world_mut()
        .get_mut::<Transform>(hull)
        .unwrap()
        .rotate_local(Quat::from_euler(EulerRot::XYZ, 0.13, 0.0, 0.07));
    app.world_mut().entity_mut(hull).insert(RigidBody::Static);
    let template = crate::damage_cam::delivery::tests::report("s1_through_crew");
    // Fixture: the box's faces are parts of one main-armour assembly in the delivery vocabulary.
    let name = template
        .plates
        .iter()
        .find(|plate| plate.class == crate::damage_cam::PlateClass::Main)
        .expect("the fixture template names main armour")
        .name
        .clone();
    for wall in &walls {
        app.world_mut()
            .entity_mut(*wall)
            .insert(Name::new(name.clone()));
    }
    settle(&mut app);
    let round_key = "88mm_AP";
    let round = crate::tank::WeaponRound::resolve(round_key).unwrap().round;
    let birth = Vec3::new(0.2, 20.5, 0.1);
    let mut fire = FireShell::of_round(
        &round,
        birth,
        birth,
        Dir3::new(Vec3::new(1.0, 0.2, 0.1)).unwrap(),
        FireMechanism::Single,
        false,
        None,
        None,
    );
    fire.speed = 30.0;
    fire.drag_k = 0.0;
    app.world_mut().trigger(fire);
    let mut records = Vec::new();
    let mut ticks = 0;
    while ticks < 64 * 10 && records.is_empty() {
        records.extend(run(&mut app, 1));
        ticks += 1;
    }
    let elapsed = ticks as f32 * TICK.as_secs_f32();
    let mut flying = app.world_mut().query::<&Projectile>();
    assert_eq!(
        flying.iter(app.world()).count(),
        0,
        "the round rests within 10 s of flight in a 2.8 m box"
    );
    let mut shells = app.world_mut().query::<&PenetrationMarks>();
    let marks = shells.single(app.world()).unwrap();
    let Some(ArmourMark::Bounce {
        speed_out,
        incoming,
        normal,
        ..
    }) = marks.log.last()
    else {
        panic!(
            "the face holds a round too slow to bite: {:?}",
            marks.log.last()
        );
    };
    assert_eq!(*speed_out, 0.0, "the face held it");
    let Some(ArmourMark::Bounce {
        speed_out: first_out,
        ..
    }) = marks.log.first()
    else {
        panic!("the fixture's first face must hold it");
    };
    assert_eq!(
        *first_out, 0.0,
        "Held ends the first contact, before a terminal skid can imitate it"
    );
    let normal_share = incoming.dot(*normal).abs();
    let sliding_share = incoming.cross(*normal).length();
    let friction = crate::physical_laws::PhysicalLaws::shipped().contact_friction;
    assert!(
        sliding_share <= friction * normal_share,
        "rest must satisfy Held, not a terminal skid: {normal_share}, {sliding_share}, {friction}"
    );
    let mut previous = birth;
    let mut distance = 0.0;
    let mut fastest = 30.0_f32;
    for mark in &marks.log {
        if let ArmourMark::Bounce { at, speed_in, .. } = mark {
            distance += at.distance(previous);
            fastest = fastest.max(*speed_in);
            previous = *at;
        }
    }
    assert!(
        elapsed >= distance / fastest,
        "{elapsed} s cannot fly {distance} m at at most {fastest} m/s"
    );
    assert_eq!(records.len(), 1, "the stopped round delivers its encounter");
    let report = &records[0].report;
    let crate::damage_cam::RoundEnd::Rest { volume } = &report.end else {
        panic!("a held round's record must end at rest");
    };
    assert!(
        report
            .plates
            .iter()
            .any(|plate| &plate.name == volume && plate.exit.is_none())
    );
    let mut report = records.remove(0).report;
    // Fixture identity comes from the loaded vocabulary; all live geometry, clocks and outcomes stay.
    // Unhit staffing belongs to the tank helper, outside this armour-only delivery fixture.
    report.components.clear();
    report.round = round_key.to_owned();
    report.projectile.name = template.projectile.name.clone();
    report.vehicle = template.vehicle.clone();
    report.servo_deg = template.servo_deg.clone();
    let vocabulary = crate::damage_cam::delivery::tests::vocabulary(&template);
    let mut delivery = App::new();
    delivery
        .add_plugins(MinimalPlugins)
        .add_plugins(crate::damage_cam::delivery::offline_plugin);
    delivery.insert_resource(vocabulary);
    delivery.insert_resource(crate::WeaponClock(crate::sim_tick::SimTick::new(0)));
    delivery
        .world_mut()
        .spawn((crate::CombatantId(9), crate::tank::Controlled));
    let recorded = crate::damage_cam::delivery::tests::recorded(
        &report,
        crate::CombatantId(7),
        1200,
        crate::damage_cam::wire::TankLife {
            combatant: crate::CombatantId(9),
            life: crate::CombatantLife(1),
        },
    );
    delivery.world_mut().write_message(recorded);
    delivery.world_mut().run_schedule(FixedUpdate);
    let store = delivery
        .world()
        .resource::<crate::damage_cam::delivery::ShotExplanations>();
    let delivered: Vec<_> = store.iter().map(|(_, stored)| &stored.report).collect();
    assert_eq!(
        delivered.len(),
        1,
        "the live box record survives real delivery"
    );
    assert_eq!(delivered[0].end, report.end);
    assert!(
        delivered[0]
            .plates
            .iter()
            .any(|plate| plate.exit.is_none() && plate.chord_mm == 0.0)
    );
}

/// The game's damage model spends real flight time between skids. The authority must
/// return to the tick clock instead of planning the unbounded future of a trapped, fuze-less
/// round. Its long open slide leaves the far wall beyond four ticks, so pre-planning that wall
/// leaves playback where discovery should have returned to the tick clock. All dimensions, speeds
/// and the 45-degree direction are fixture data.
#[test]
fn a_closed_box_of_faces_returns_to_the_flight_clock() {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    let (hull, _) = tank(
        &mut app,
        Vec3::Y * 20.0,
        0.0,
        Vec3::ZERO,
        &[
            (
                "right",
                Vec3::new(0.4, 4.0, 40.0),
                Vec3::X * 1.2,
                Kind::Plate,
            ),
            (
                "left",
                Vec3::new(0.4, 4.0, 40.0),
                Vec3::NEG_X * 1.2,
                Kind::Plate,
            ),
            (
                "front",
                Vec3::new(2.0, 4.0, 0.4),
                Vec3::Z * 20.2,
                Kind::Plate,
            ),
            (
                "back",
                Vec3::new(2.0, 4.0, 0.4),
                Vec3::NEG_Z * 20.2,
                Kind::Plate,
            ),
        ],
    );
    app.world_mut().entity_mut(hull).insert(RigidBody::Static);
    settle(&mut app);
    let round = crate::tank::WeaponRound::resolve("7.9mm_AP").unwrap().round;
    let birth = Vec3::new(0.2, 20.0, 0.0);
    let mut fire = FireShell::of_round(
        &round,
        birth,
        birth,
        Dir3::new(Vec3::new(1.0, 0.0, 1.0)).unwrap(),
        FireMechanism::Single,
        false,
        None,
        None,
    );
    fire.speed = 100.0;
    fire.drag_k = 0.0;
    app.world_mut().trigger(fire);
    let records = run(&mut app, 4);
    assert!(records.is_empty(), "four ticks leave the round in flight");
    let mut shells = app.world_mut().query::<&PenetrationMarks>();
    let marks = shells.single(app.world()).unwrap();
    assert!(!marks.ricochets.is_empty(), "the fixture skids");
    let elapsed = 4.0 * TICK.as_secs_f32();
    let mut previous = birth;
    let mut distance = 0.0;
    let mut fastest = 100.0_f32;
    for mark in &marks.log {
        if let ArmourMark::Bounce {
            at,
            speed_in,
            speed_out,
            incoming,
            normal,
            ..
        } = mark
        {
            distance += at.distance(previous);
            previous = *at;
            fastest = fastest.max(*speed_in);
            let normal_share = incoming.dot(*normal).abs();
            let friction = crate::physical_laws::PhysicalLaws::shipped().contact_friction;
            assert!(
                incoming.cross(*normal).length() > friction * normal_share,
                "this fixture's reached faces permit flight"
            );
            assert!(*speed_out > 0.0, "a permitted skid keeps its motion");
        }
    }
    assert!(
        distance <= fastest * elapsed,
        "{distance} m exceeds {elapsed} s of flight"
    );
    let mut flying = app.world_mut().query::<&Projectile>();
    let projectile = flying
        .single(app.world())
        .expect("four ticks leave one projectile in flight");
    assert!(
        projectile.playback.is_empty(),
        "the open slide returns discovery to the tick clock"
    );
    assert!(projectile.velocity.is_finite());
    assert!(projectile.velocity.length() > 0.0);
}

/// The game's damage model delivers the resolved disc contacts through production playback.
/// Construction consumers rely on the off-axis chords surviving ShellMarched, while public
/// crossing endpoints stay on the shared axis. Geometry and firing distance are fixture data.
#[test]
fn delivered_lines_keep_the_disc_contacts_beside_the_shared_axis() {
    #[derive(Resource, Default)]
    struct Lines(Vec<DeliveredLine>);
    let mut app = recording_world(false);
    app.init_resource::<Lines>().add_observer(
        |event: On<ShellMarched>, mut lines: ResMut<Lines>| {
            lines.0.extend_from_slice(&event.lines);
        },
    );
    tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[("plate", Vec3::new(3.0, 3.0, THIN), Vec3::ZERO, Kind::Plate)],
    );
    fire(&mut app, Vec3::Z * 4.0, FireMechanism::Single);
    for _ in 0..4 {
        app.update();
    }
    let lines = &app.world().resource::<Lines>().0;
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    assert!(
        line.contacts.len() > 1,
        "the fixture disc has covered rim samples"
    );
    assert!(
        line.contacts
            .iter()
            .any(|(a, _)| (a.x - line.crossing.entry.x).abs() > 1e-3)
    );
    let mean = line
        .contacts
        .iter()
        .map(|(a, b)| a.distance(*b))
        .sum::<f32>()
        / line.contacts.len() as f32;
    let capacity = crate::physical_laws::PhysicalLaws::shipped()
        .plate_capacity
        .charge(f64::from(THIN) / f64::from(FIXTURE_88.caliber));
    assert!(
        (mean * capacity as f32 - line.crossing.chord).abs() < 1e-4,
        "contacts keep real geometry while the readout chord carries the work share"
    );
}

/// Fixture face, normal `+Z`, and the round's incidence on it from that normal: at 75° the fixture
/// 88 skids at the fixture speed below.
const SKID_FACE: Vec3 = Vec3::new(40.0, 40.0, 0.3);
const SKID_INCIDENCE_DEG: f32 = 75.0;
const SKID_SPEED: f32 = 773.0;

/// What one oblique strike came to: its one bounce mark, and the round at the end of the run.
#[derive(Debug)]
struct Skid {
    bounces: Vec<(f32, f32, f32, Vec3)>,
    end: Vec3,
    /// `None`: the round came to rest.
    velocity: Option<Vec3>,
    /// Each bounce mark's axis point, and the path node its [`RicochetMark`] names.
    turns: Vec<(Vec3, Vec3)>,
}

/// A drag-free fixture 88 meets [`SKID_FACE`], at rest or moving at `face`, with the relative
/// velocity [`SKID_SPEED`] at [`SKID_INCIDENCE_DEG`]; `elsewhere` is the velocity of a body far off
/// the round's path. Incoming speed and shock are relative to the face; outgoing speed and
/// direction are world values. The end velocity has `face` taken off.
fn skid_on(face: Vec3, elsewhere: Option<Vec3>) -> Skid {
    skid_from(face, elsewhere, 6.0)
}

fn skid_from(face: Vec3, elsewhere: Option<Vec3>, approach: f32) -> Skid {
    let mut app = recording_world(false);
    app.insert_resource(RetainSpentShells(true));
    let (root, _) = tank(
        &mut app,
        Vec3::ZERO,
        0.0,
        Vec3::ZERO,
        &[("face", SKID_FACE, Vec3::ZERO, Kind::Plate)],
    );
    let mover = elsewhere.map(|_| {
        tank(
            &mut app,
            Vec3::Y * 200.0,
            0.0,
            Vec3::ZERO,
            &[("elsewhere", Vec3::splat(2.0), Vec3::ZERO, Kind::Plate)],
        )
        .0
    });
    settle(&mut app);
    app.world_mut()
        .entity_mut(root)
        .insert(LinearVelocity(face));
    if let (Some(mover), Some(velocity)) = (mover, elsewhere) {
        app.world_mut()
            .entity_mut(mover)
            .insert(LinearVelocity(velocity));
    }
    let incidence = SKID_INCIDENCE_DEG.to_radians();
    let relative = Vec3::new(incidence.sin(), 0.0, -incidence.cos()) * SKID_SPEED;
    let velocity = relative + face;
    let strike = Vec3::Z * (SKID_FACE.z * 0.5);
    let from = strike - relative.normalize() * approach;
    let mut fire = fixture_fire(
        &mut app,
        from,
        Dir3::new(velocity).unwrap(),
        FireMechanism::Single,
        None,
        None,
    );
    fire.speed = velocity.length();
    fire.drag_k = 0.0;
    app.world_mut().trigger(fire);
    run(&mut app, 4);
    let mut shells = app.world_mut().query::<(
        &Transform,
        Option<&Projectile>,
        &PenetrationMarks,
        &ShellPath,
    )>();
    let (transform, projectile, marks, path) = shells.single(app.world()).unwrap();
    Skid {
        turns: marks
            .log
            .iter()
            .filter_map(|mark| match mark {
                ArmourMark::Bounce { at, .. } => Some(*at),
                _ => None,
            })
            .zip(&marks.ricochets)
            .map(|(at, ricochet)| (at, path.points[ricochet.path_index]))
            .collect(),
        bounces: marks
            .log
            .iter()
            .filter_map(|mark| match mark {
                ArmourMark::Bounce {
                    speed_in,
                    speed_out,
                    shock,
                    outgoing,
                    ..
                } => Some((*speed_in, *speed_out, *shock, *outgoing)),
                _ => None,
            })
            .collect(),
        end: transform.translation,
        velocity: projectile.map(|projectile| projectile.velocity - face),
    }
}

/// The game's damage model: unrelated motion in the world changes no shot's outcome. Every player
/// relies on it: whether a round skids off a parked hull, and what it keeps, cannot depend on
/// whether another tank is driving. Mutant: a contact placed at first touch has its resting height
/// behind it, ends the round at the face and charges it `½ m v²` where the law says
/// `½ m (v cos θ)²`, only while something moves.
#[test]
fn unrelated_motion_changes_no_shots_outcome() {
    let at_rest = skid_on(Vec3::ZERO, None);
    assert_eq!(
        at_rest.bounces.len(),
        1,
        "the fixture skids once: {at_rest:?}"
    );
    let (speed_in, speed_out, shock, _) = at_rest.bounces[0];
    let incidence = SKID_INCIDENCE_DEG.to_radians();
    assert!(
        (speed_in - SKID_SPEED).abs() < 1.0,
        "the bounce reads the striking speed: {speed_in}"
    );
    assert!(
        (speed_out - speed_in * incidence.sin()).abs() < 1.0,
        "the skid keeps the speed along the face: {speed_out}"
    );
    let moving = skid_on(Vec3::ZERO, Some(Vec3::X * 20.0));
    assert_eq!(
        at_rest.bounces, moving.bounces,
        "a body driving elsewhere leaves the skid alone"
    );
    assert_eq!(at_rest.velocity, moving.velocity, "and what it keeps");
    assert_eq!(at_rest.end, moving.end, "and where it ends");
    assert!(shock > 0.0);
}

/// The game's damage model: a skid's mark names the axis node it turned at. The trail follows the
/// path by that place (`vfx::trail`), and a coordinate join misses it: the axis turns a body radius
/// off the strike. Mutant: the mark names any node but the skid's own.
#[test]
fn a_skid_names_its_turn_in_the_shell_path() {
    let skid = skid_on(Vec3::ZERO, None);
    assert_eq!(skid.turns.len(), 1, "{skid:?}");
    let (at, node) = skid.turns[0];
    assert_eq!(at, node);
}

/// The game's damage model: a moving face decides a strike in its own frame. A round's verdict, the
/// speed it keeps and its way out rely on the relative velocity alone, so a player's shot at a
/// driving tank obeys the same skid as at a parked one. Mutant: a first touch resolved with no
/// approach left ends the round at the face. A mark's outgoing motion is the world's, so the face's
/// velocity comes off it. The fixture's face velocities lie along and into it.
#[test]
fn a_moving_face_decides_a_skid_in_its_own_frame() {
    let at_rest = skid_on(Vec3::ZERO, None);
    let (_, kept, jarred, out) = at_rest.bounces[0];
    let flying_on = at_rest.velocity.expect("a skid carries the round on");
    for face in [Vec3::X * 20.0, Vec3::Z * 8.0, Vec3::new(-15.0, 0.0, 6.0)] {
        let moving = skid_on(face, None);
        assert_eq!(moving.bounces.len(), 1, "{face}: {moving:?}");
        let (speed_in, speed_out, shock, outgoing) = moving.bounces[0];
        assert!(
            (speed_in - SKID_SPEED).abs() < 0.5,
            "{face}: the frame's striking speed {speed_in}"
        );
        let leaving = outgoing * speed_out - face;
        assert!(
            (leaving - out * kept).length() < 1.0,
            "{face}: the frame's way out {leaving} against {}",
            out * kept
        );
        assert!(
            (shock / jarred - 1.0).abs() < 0.01,
            "{face}: the frame's shock {shock} against {jarred}"
        );
        let end = moving.velocity.expect("a skid carries the round on");
        assert!(
            (end - flying_on).length() < 1.0,
            "{face}: the end velocity in the face's frame {end} against {flying_on}",
        );
    }
}

/// The game's damage model: an instant contact burst reaches the moving face's world position.
/// The burst mark, fragment field and environment blast share that point. Fixture speeds include
/// a spent round carried almost alongside a fast face; a small relative speed amplifies stand-off
/// seconds. Mutant: apply the carrier correction only after recording and throwing the burst.
#[test]
fn an_instant_contact_burst_moves_with_its_carrier_before_delivery() {
    for (relative_speed, carrier_speed) in
        [(773.0, 15.0), (100.0, 15.0), (1.0, 100.0), (0.1, 100.0)]
    {
        let mut app = recording_world(false);
        app.insert_resource(RetainSpentShells(true));
        let (hull, _) = tank(
            &mut app,
            Vec3::ZERO,
            0.0,
            Vec3::ZERO,
            &[("face", SKID_FACE, Vec3::ZERO, Kind::Plate)],
        );
        settle(&mut app);
        app.world_mut()
            .entity_mut(hull)
            .insert(LinearVelocity(Vec3::Z * carrier_speed));
        let seconds = TICK.as_secs_f32() * 0.75;
        let birth = Vec3::Z * (SKID_FACE.z * 0.5 + relative_speed * seconds);
        let velocity = Vec3::Z * (carrier_speed - relative_speed);
        let inputs = burst::BurstInputs {
            trigger: crate::rounds::fuze::Trigger::Impact { sensitivity_n: 0.0 },
            delay: 0.0,
            ..burst::fixture_inputs()
        };
        let burst = burst::Burst::derived(FIXTURE_88, burst::FIXTURE_STEEL, &inputs).unwrap();
        let mut fire = fixture_fire(
            &mut app,
            birth,
            Dir3::new(velocity).unwrap(),
            FireMechanism::Single,
            None,
            Some(burst),
        );
        fire.speed = velocity.length();
        fire.drag_k = 0.0;
        app.world_mut().trigger(fire);
        run(&mut app, 1);
        let mut shells = app.world_mut().query::<(&PenetrationMarks, &SpallMarks)>();
        let (marks, fields) = shells.single(app.world()).unwrap();
        let at = marks
            .log
            .iter()
            .find_map(|mark| match mark {
                ArmourMark::Burst { at, .. } => Some(*at),
                _ => None,
            })
            .expect("the fixture functions at contact");
        let expected = SKID_FACE.z * 0.5 + carrier_speed * seconds;
        let error = (at.z - expected).abs();

        // Fixture slack: the incoming disc's gravity tilt puts its first rim under 0.1 mm ahead
        // of its axis; f32 pose/cast rounding fits within another 0.1 mm.
        assert!(
            error < 0.0002,
            "the world contact, not the earlier frozen frame, owns the burst"
        );
        assert_eq!(fields.burst.as_ref().unwrap().origin, at);
    }
}

/// The game's damage model: tick placement must not turn a valid skid into entry.
/// Fixture: first touch 5 mm ahead, inside the DERIVED 9.66 mm reopening band for this disc.
/// Still and moving faces need the same reached-contact identity; extending behind the spawn
/// would rewind unpaid time. The owner's contact rebuild owns this finding (#260).
#[test]
#[ignore = "#260: tick-boundary first touch needs the contact-state rebuild"]
fn a_tick_boundary_does_not_turn_a_skid_into_entry() {
    let cases: Vec<_> = [Vec3::ZERO, Vec3::Z * 8.0]
        .into_iter()
        .map(|face| (face, skid_on(face, None), skid_from(face, None, 0.005)))
        .collect();
    for (face, reference, near) in &cases {
        assert!(
            near.velocity.is_some(),
            "both frames were exercised: {cases:?}"
        );
        assert_eq!(
            near.bounces.len(),
            reference.bounces.len(),
            "{face}: {near:?}"
        );
        assert!(
            (near.velocity.unwrap() - reference.velocity.unwrap()).length() < 1.0,
            "{face}: a tick boundary changes no strike's outgoing motion"
        );
    }
}
