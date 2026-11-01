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
        effects: crate::rounds::FlightEffects::fixture_traced(),
        muzzle: crate::guns::MuzzleEmission::fixture_cannon(),
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
        tick: wait.ticks(),
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
/// being read as a broken walk. A swept start inside a collider remains a bug (`motion`'s
/// `discovery_refuses_a_shallow_inside_start`). Mutant: laying the birth disc as swept panics.
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
    app.insert_resource(crate::ShotClock(100))
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
