//! A measurement bed for the game's environment destruction (game state only): the shipped house
//! felled on the flat world at its ground supports (`construction::breakage::felling`), its rubble
//! left to rest and freeze (`debris::freeze`), then driven into by the shipped tank or reached by
//! a fixture charge. Every tick it checks the freeze against the world itself: no touching pair
//! joins a frozen piece to a body that is not static, the tank's belt never stands on a frozen
//! piece, and every frozen island's support stands. It prints counts, pins none of them, and runs
//! on demand.

use super::*;
use crate::ballistics::BallisticVolume;
use crate::construction::breakage::{Breakage, Charge, Piece, felling};
use crate::construction::fracture::{Blast, FIT_ONLY_PRODUCTS};
use crate::construction::{Body, Element};
use crate::cost::stages::{Stages, Work};
use crate::debris::freeze::{Frozen, FrozenIslands};
use avian3d::prelude::Sleeping;
use avian3d::prelude::{ColliderOf, ContactGraph, LinearVelocity, Position, RigidBody, Rotation};
use bevy::ecs::system::RunSystemOnce;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// The fixture house's map ordinal: no shipped instance's.
const ORDINAL: u16 = 60000;
/// Fixture: from the tank's centre to the house's origin beyond the run-up, m; the pile keeps
/// clear of the waiting tank.
const HOUSE_BACK_M: f32 = 15.0;
/// Fixture: the charge case's house, ahead of the tank, m.
const CHARGE_HOUSE_M: f32 = 40.0;
/// Fixture: the 88 mm HE burst's blast energy, J (`construction::breakage` tests' burst).
const BURST_J: f32 = 1.47e6;
/// Fixture: the charge's height over the pile's centre, m.
const CHARGE_ABOVE_M: f32 = 1.0;
/// Bounds on each phase, s: the rubble's rest, the drive, the brake and the settle after.
const REST_S: usize = 60;
const DRIVE_S: usize = 30;
const BRAKE_S: usize = 3;
const SETTLE_S: usize = 30;
/// Fixture: the tank stops driving this far past the pile's farthest piece, m.
const PAST_M: f32 = 10.0;

#[derive(Clone, Copy, Debug)]
enum Strike {
    /// The tank drives full throttle at a house `offset_m` to its left, `runup_m` plus
    /// [`HOUSE_BACK_M`] ahead.
    Drive { offset_m: f32, runup_m: f32 },
    /// A fixture burst's blast over the pile.
    Charge,
}

/// One tick's resting census of the fixture house's pieces.
#[derive(Clone, Copy, Default)]
struct Census {
    awake: usize,
    sleeping: usize,
    frozen: usize,
}

/// One tick's state for a determinism comparison: the tank's pose bits, and each piece's pose
/// bits and whether it is frozen and asleep, in entity order.
type Digest = (Vec<u32>, Vec<([u32; 7], bool, bool)>);

/// What one run saw.
#[derive(Default)]
struct Log {
    tick: u32,
    rested: Option<u32>,
    at_rest: Census,
    first_contact: Option<(u32, f32, f32, usize)>,
    first_thaw: Option<u32>,
    frozen_before_thaw: usize,
    last_frozen: usize,
    froze: u64,
    refroze: u64,
    thawed: u64,
    by_reach: u64,
    by_load: u64,
    by_support: u64,
    met: u64,
    touched_frozen: u32,
    first_touched_frozen: Option<String>,
    belt_on_frozen: u32,
    unsupported: u32,
    first_unsupported: Option<String>,
    end: Census,
    digests: Option<Vec<Digest>>,
}

fn pose_bits(position: &Position, rotation: &Rotation) -> [u32; 7] {
    let (p, r) = (position.0, rotation.0);
    [p.x, p.y, p.z, r.x, r.y, r.z, r.w].map(f32::to_bits)
}

/// Record the tick just run.
fn observe(app: &mut App, tank: Entity, stages: &Stages, log: &mut Log) -> Census {
    let counts = stages.take_counts().values;
    let work = |work: Work| counts[work as usize];
    let world = app.world_mut();
    let mut pieces = world.query::<(
        Entity,
        &Piece,
        Has<Frozen>,
        Has<Sleeping>,
        &Position,
        &Rotation,
    )>();
    let mut pile: Vec<_> = pieces
        .iter(world)
        .filter(|(_, piece, ..)| piece.construction == ORDINAL)
        .collect();
    pile.sort_unstable_by_key(|(entity, ..)| *entity);
    let mut census = Census::default();
    for &(_, _, frozen, sleeping, ..) in &pile {
        match (frozen, sleeping) {
            (true, true) => panic!("a frozen piece is not asleep"),
            (true, false) => census.frozen += 1,
            (false, true) => census.sleeping += 1,
            (false, false) => census.awake += 1,
        }
    }
    let is_frozen = |body: Entity| world.get::<Frozen>(body).is_some();
    let is_static = |body: Entity| {
        world
            .get::<RigidBody>(body)
            .is_some_and(RigidBody::is_static)
    };
    let in_pile = |body: Entity| {
        world
            .get::<Piece>(body)
            .is_some_and(|piece| piece.construction == ORDINAL)
    };
    let graph = world.resource::<ContactGraph>();
    let mut contact = false;
    for pair in graph
        .iter_active_touching()
        .chain(graph.iter_sleeping_touching())
        .filter(|pair| pair.generates_constraints())
    {
        let (Some(body1), Some(body2)) = (pair.body1, pair.body2) else {
            continue; // A collider without a body is no piece.
        };
        for (piece, other) in [(body1, body2), (body2, body1)] {
            if is_frozen(piece) && !is_static(other) {
                log.touched_frozen += 1;
                log.first_touched_frozen.get_or_insert_with(|| {
                    format!("tick {}: frozen {piece} touches {other}", log.tick)
                });
            }
            contact |= in_pile(piece) && !is_static(other) && !in_pile(other);
        }
    }
    let belt = world
        .get::<crate::track::sim::TrackContacts>(tank)
        .expect("the tank has belt contacts");
    if belt
        .0
        .iter()
        .flatten()
        .filter_map(|c| c.collider)
        .any(|collider| {
            world
                .get::<ColliderOf>(collider)
                .is_some_and(|of| is_frozen(of.body))
        })
    {
        log.belt_on_frozen += 1;
    }
    let lost = world.resource::<FrozenIslands>().unsupported(world);
    if !lost.is_empty() {
        log.unsupported += 1;
        log.first_unsupported
            .get_or_insert_with(|| format!("tick {}: {lost:?}", log.tick));
    }
    if contact && log.first_contact.is_none() {
        let speed = world.get::<LinearVelocity>(tank).unwrap().0.length();
        let z = world.get::<Position>(tank).unwrap().0.z;
        log.first_contact = Some((log.tick, speed, z, census.frozen));
    }
    let thawed = work(Work::ThawedPieces);
    if thawed > 0 && log.first_thaw.is_none() {
        log.first_thaw = Some(log.tick);
        log.frozen_before_thaw = log.last_frozen;
    }
    let froze = work(Work::FrozePieces);
    log.froze += froze;
    if log.first_thaw.is_some() {
        log.refroze += froze;
    }
    log.thawed += thawed;
    log.by_reach += work(Work::ThawReach);
    log.by_load += work(Work::ThawLoad);
    log.by_support += work(Work::ThawSupport);
    log.met += work(Work::FreezeMet);
    if let Some(digests) = &mut log.digests {
        let tank_pose = pose_bits(
            world.get::<Position>(tank).unwrap(),
            world.get::<Rotation>(tank).unwrap(),
        );
        let pieces = pile
            .iter()
            .map(|&(_, _, frozen, sleeping, position, rotation)| {
                (pose_bits(position, rotation), frozen, sleeping)
            })
            .collect();
        digests.push((tank_pose.to_vec(), pieces));
    }
    log.last_frozen = census.frozen;
    log.tick += 1;
    census
}

/// Spawn the shipped house at `at` and fell it at its ground supports.
fn fell_house(app: &mut App, at: Vec3) {
    let kind = crate::construction::tests::shipped_house();
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, app.world());
    let root = crate::construction::spawn(
        &mut commands,
        &kind,
        crate::scatter::MapInstance {
            id: "rubble_fixture".to_owned(),
            ordinal: ORDINAL,
        },
        Isometry3d::from_translation(at),
    );
    queue.apply(app.world_mut());
    app.update();
    app.world_mut()
        .run_system_once(
            move |houses: Query<(&Transform, &Breakage), With<Body>>,
                  volumes: Query<(Entity, &ChildOf, &Element), With<BallisticVolume>>,
                  mut commands: Commands| {
                let (pose, body) = houses.get(root).expect("the fixture house stands");
                let supports = felling::supports(body);
                let lines =
                    felling::felling_lines("rubble_fixture", root, pose, body, &supports, &volumes);
                assert!(!lines.is_empty(), "the shipped house stands on supports");
                felling::fell(&mut commands, lines);
            },
        )
        .expect("the felling runs");
}

/// The pile's frozen pieces' positions.
fn frozen_positions(app: &mut App) -> Vec<Vec3> {
    let world = app.world_mut();
    let mut pieces = world.query_filtered::<(&Piece, &Position), With<Frozen>>();
    pieces
        .iter(world)
        .filter(|(piece, _)| piece.construction == ORDINAL)
        .map(|(_, position)| position.0)
        .collect()
}

/// The shipped tank parked at the origin of a flat world with no placement, facing +z.
fn flat_world() -> (BootedSim, Entity) {
    let mut app = booted_driving_grid(driving_plane(0.0));
    start_fixed_clock(&mut app);
    app.update(); // The physics body exists before the fixture places it.
    let tank = app
        .world_mut()
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>()
        .single(app.world())
        .expect("one controlled tank");
    let rest = app
        .world()
        .resource::<crate::track::rig_geom::RigGeom>()
        .hull_rest_y;
    let facing = Quat::from_rotation_y(std::f32::consts::PI);
    super::drive_load::place(&mut app, tank, Vec3::Y * rest, facing, 0.0, 1);
    for _ in 0..ticks(2) {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    (app, tank)
}

fn ticks(s: usize) -> usize {
    s * FIXED_TICKS_PER_SECOND
}

fn run(strike: Strike, shift: bool, digest: bool) -> Log {
    let (mut app, tank) = flat_world();
    let stages = Stages::default();
    app.insert_resource(stages.clone());
    if shift {
        app.world_mut()
            .spawn_batch((0..1000).map(|_| Name::new("shift")));
    }
    let start = app.world().get::<Position>(tank).unwrap().0;
    let (offset, ahead) = match strike {
        Strike::Drive { offset_m, runup_m } => (offset_m, runup_m + HOUSE_BACK_M),
        Strike::Charge => (0.0, CHARGE_HOUSE_M),
    };
    fell_house(&mut app, Vec3::new(start.x - offset, 0.0, start.z + ahead));
    let mut log = Log {
        digests: digest.then(Vec::new),
        ..default()
    };
    stages.take_counts();
    let mut quiet = 0;
    for _ in 0..ticks(REST_S) {
        drive_tick(&mut app, tank, 0.0, 0.0);
        let census = observe(&mut app, tank, &stages, &mut log);
        quiet = if census.awake == 0 && census.sleeping == 0 && census.frozen > 0 {
            quiet + 1
        } else {
            0
        };
        if quiet >= ticks(2) {
            log.rested = Some(log.tick);
            break;
        }
    }
    log.at_rest = Census {
        frozen: log.last_frozen,
        ..observe_counts(&mut app)
    };
    match strike {
        Strike::Drive { .. } => {
            let far = frozen_positions(&mut app)
                .iter()
                .map(|p| p.z)
                .fold(f32::NEG_INFINITY, f32::max);
            for _ in 0..ticks(DRIVE_S) {
                drive_tick(&mut app, tank, 1.0, 0.0);
                observe(&mut app, tank, &stages, &mut log);
                if app.world().get::<Position>(tank).unwrap().0.z > far + PAST_M {
                    break;
                }
            }
            for _ in 0..ticks(BRAKE_S) {
                let forward = app.world().get::<LinearVelocity>(tank).unwrap().0.z
                    > crate::track::transmission::PARK_ENGAGE_SPEED;
                drive_tick(&mut app, tank, if forward { -1.0 } else { 0.0 }, 0.0);
                observe(&mut app, tank, &stages, &mut log);
            }
        }
        Strike::Charge => {
            let pile = frozen_positions(&mut app);
            let centre = pile.iter().sum::<Vec3>() / pile.len() as f32;
            app.world_mut().trigger(Charge {
                at: centre + Vec3::Y * CHARGE_ABOVE_M,
                blast: Blast::of(BURST_J, 1e-3, FIT_ONLY_PRODUCTS),
            });
        }
    }
    for _ in 0..ticks(SETTLE_S) {
        drive_tick(&mut app, tank, 0.0, 0.0);
        log.end = observe(&mut app, tank, &stages, &mut log);
    }
    log
}

/// The pile's awake and sleeping pieces now.
fn observe_counts(app: &mut App) -> Census {
    let world = app.world_mut();
    let mut pieces = world.query::<(&Piece, Has<Frozen>, Has<Sleeping>)>();
    let mut census = Census::default();
    for (piece, frozen, sleeping) in pieces.iter(world) {
        if piece.construction != ORDINAL {
            continue;
        }
        census.frozen += usize::from(frozen);
        census.sleeping += usize::from(sleeping && !frozen);
        census.awake += usize::from(!sleeping && !frozen);
    }
    census
}

/// Run `strike`, catching a panic as its outcome.
fn attempt(strike: Strike, shift: bool, digest: bool) -> Result<Log, String> {
    catch_unwind(AssertUnwindSafe(|| run(strike, shift, digest))).map_err(|panic| {
        panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "a panic without a message".to_owned())
    })
}

fn report(name: &str, outcome: &Result<Log, String>) {
    let log = match outcome {
        Ok(log) => log,
        Err(panic) => {
            println!("MEASURED frozen rubble, {name}: PANICKED: {panic}");
            return;
        }
    };
    let dt = 1.0 / FIXED_TICKS_PER_SECOND as f32;
    let seconds = |tick: Option<u32>| tick.map(|t| format!("{:.2} s", t as f32 * dt));
    println!(
        "MEASURED frozen rubble, {name}: rested {:?} with {} frozen, {} sleeping, {} awake; \
         first contact {:?} (tick, speed m/s, tank z, frozen then); first thaw {:?} with {} \
         frozen the tick before; froze {} pieces, thawed {} pieces (islands: {} by reach, {} by \
         load, {} by support), refroze {} after the first thaw; sleeping islands that met a \
         frozen one: {}; ticks a moving body touched a frozen piece: {} ({:?}); ticks the belt stood on one: {}; ticks an island lacked \
         support: {} ({:?}); at the end {} frozen, {} sleeping, {} awake",
        seconds(log.rested),
        log.at_rest.frozen,
        log.at_rest.sleeping,
        log.at_rest.awake,
        log.first_contact,
        seconds(log.first_thaw),
        log.frozen_before_thaw,
        log.froze,
        log.thawed,
        log.by_reach,
        log.by_load,
        log.by_support,
        log.refroze,
        log.met,
        log.touched_frozen,
        log.first_touched_frozen,
        log.belt_on_frozen,
        log.unsupported,
        log.first_unsupported,
        log.end.frozen,
        log.end.sleeping,
        log.end.awake,
    );
}

/// The game's environment destruction: rubble frozen at rest thaws whole before a tank or a
/// blast can meet it as fixed matter, and stays supported while it is frozen. Fixture offsets and
/// run-ups; the counts are observations.
#[test]
#[ignore = "a measurement bed, run on demand"]
fn frozen_rubble_meets_a_tank_and_a_charge() {
    let mut strikes: Vec<(String, Strike)> = [-3.0, 0.0, 3.0]
        .into_iter()
        .flat_map(|offset_m| {
            [2.0, 20.0, 80.0].map(|runup_m| {
                (
                    format!("drive, offset {offset_m} m, run-up {runup_m} m"),
                    Strike::Drive { offset_m, runup_m },
                )
            })
        })
        .collect();
    strikes.push(("charge".to_owned(), Strike::Charge));
    let outcomes: Vec<_> = strikes
        .iter()
        .map(|(name, strike)| {
            let outcome = attempt(*strike, false, false);
            report(name, &outcome);
            (name, outcome)
        })
        .collect();
    for (name, outcome) in &outcomes {
        let log = outcome
            .as_ref()
            .unwrap_or_else(|panic| panic!("{name} panicked: {panic}"));
        assert!(
            log.first_thaw.is_some(),
            "{name}: the strike thawed the pile"
        );
        assert_eq!(
            log.touched_frozen, 0,
            "{name}: a moving body touched a frozen piece"
        );
        assert_eq!(
            log.belt_on_frozen, 0,
            "{name}: the belt stood on a frozen piece"
        );
        assert_eq!(
            log.unsupported, 0,
            "{name}: a frozen island lost its support"
        );
    }
}

/// The game's environment destruction: the same thaw, run twice and once with every entity's
/// bits shifted in order, repeats every pose bit and every freeze and thaw to the end.
#[test]
#[ignore = "a measurement bed, run on demand"]
fn frozen_rubble_thaws_the_same_way_every_run() {
    let strike = Strike::Drive {
        offset_m: 0.0,
        runup_m: 20.0,
    };
    let runs = [("first", false), ("second", false), ("shifted", true)].map(|(name, shift)| {
        let outcome = attempt(strike, shift, true);
        report(name, &outcome);
        (
            name,
            outcome.unwrap_or_else(|panic| panic!("{name} panicked: {panic}")),
        )
    });
    let reference = runs[0].1.digests.as_ref().unwrap();
    for (name, log) in &runs[1..] {
        let digests = log.digests.as_ref().unwrap();
        let first = reference.iter().zip(digests).position(|(a, b)| a != b);
        println!(
            "MEASURED frozen rubble determinism, {name} against first: {} ticks against {}, \
             first differing tick {first:?}, first thaw at tick {:?}",
            digests.len(),
            reference.len(),
            log.first_thaw,
        );
        assert_eq!(first, None, "{name} differs from the first run");
        assert_eq!(digests.len(), reference.len(), "{name} ran as long");
    }
}
