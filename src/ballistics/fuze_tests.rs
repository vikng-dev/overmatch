//! The game's damage model: the fuze in the march. An `Impact` fuze functions at contact with any
//! solid body, the ground included, and bursts its mode's delay of flight later; the `SpendExit`
//! fuze the 8.8 cm Pzgr. 39 ring carries keeps today's outcomes.
use super::march_tests::{flat_grid, spawn_round, world_with_plate};
use super::*;
use crate::rounds::fuze::Trigger;
use crate::substances::SubstanceRegistry;

// Fixture data: a 0.4 m slab centred 4 m down range of the muzzle, the 88 mm fixture round at
// 800 m/s.
const ORIGIN: Vec3 = Vec3::new(0.0, 2.0, 2.0);
const PLATE: Vec3 = Vec3::new(0.0, 2.0, -2.0);
const THICKNESS: f32 = 0.4;
const SPEED: f32 = 800.0;
/// Fixture data: a sensitivity of the shipped high-explosive round's magnitude (N).
const SENSITIVITY_N: f32 = 30.0;
/// Fixture data: the shipped `delay` mode's delay (s).
const DELAY_S: f32 = 0.1;

fn impact(delay: f32) -> burst::Burst {
    let mut inputs = burst::fixture_inputs();
    inputs.trigger = Trigger::Impact {
        sensitivity_n: SENSITIVITY_N,
    };
    inputs.delay = delay;
    burst::Burst::derived(march_tests::FIXTURE_88, burst::FIXTURE_STEEL, &inputs).unwrap()
}

fn shot(app: &mut App, origin: Vec3, velocity: Vec3, burst: burst::Burst) -> Entity {
    app.insert_resource(RetainSpentShells(true));
    let shell = spawn_round(app, march_tests::FIXTURE_88, origin, velocity);
    let mut round = app.world_mut().get_mut::<Projectile>(shell).unwrap();
    round.fuze = burst::Fuze::of(Some(burst));
    // No drag: the fuze's clock is read off the flight without the drag law in it.
    round.drag_k = 0.0;
    shell
}

/// A world with the fixture slab made of the shipped substance `substance`.
fn slab_of(substance: &str) -> App {
    let mut app = world_with_plate(Vec3::new(3.0, 3.0, THICKNESS), PLATE);
    let material = walk::VolumeMaterial::of(SubstanceRegistry::shipped_static(), substance)
        .expect("a shipped hard substance");
    let world = app.world_mut();
    for mut volume in world.query::<&mut BallisticVolume>().iter_mut(world) {
        volume.material = material;
    }
    app
}

fn advance(app: &mut App) {
    for _ in 0..16 {
        app.update();
    }
}

/// The shell's one burst: where, at what speed, and how long it waited at rest.
fn burst_mark(app: &App, shell: Entity) -> (Vec3, f32, f32) {
    let bursts: Vec<_> = app
        .world()
        .get::<PenetrationMarks>(shell)
        .unwrap()
        .log
        .iter()
        .filter_map(|mark| match mark {
            ArmourMark::Burst {
                at, speed, after, ..
            } => Some((*at, *speed, *after)),
            _ => None,
        })
        .collect();
    assert_eq!(bursts.len(), 1, "one burst: {bursts:?}");
    bursts[0]
}

/// The game's damage model: an `Impact` fuze in its instant mode bursts at the struck face, in
/// flight, on a body as weak as the shipped wood and as strong as the shipped armour alike: its
/// resisting force at contact exceeds any sensitivity of newtons. The high-explosive round's
/// instant mode relies on it. A trigger read at the exit, or a force that never reaches the fuze,
/// moves the burst off the face or loses it.
#[test]
fn an_impact_fuze_functions_at_contact_on_wood_and_on_armour() {
    for substance in ["Wood", "RHA"] {
        let mut app = slab_of(substance);
        let shell = shot(&mut app, ORIGIN, Vec3::NEG_Z * SPEED, impact(0.0));
        advance(&mut app);
        let (at, speed, after) = burst_mark(&app, shell);
        let face = PLATE.z + 0.5 * THICKNESS;
        assert!(
            (at.z - face).abs() < 1e-4,
            "{substance}: at the face, not {at}"
        );
        assert!(
            speed > 0.99 * SPEED,
            "{substance}: in flight, at {speed} m/s"
        );
        assert_eq!(after, 0.0, "{substance}");
    }
}

/// The game's damage model: the ground functions an `Impact` fuze. It stops the round dead, so the
/// instant mode bursts on its surface and the delay mode waits out its whole delay there. The
/// high-explosive round's ground bursts rely on it; a ground branch that never reached the fuze
/// leaves the round with no burst at all.
#[test]
fn the_ground_functions_an_impact_fuze_in_either_mode() {
    for delay in [0.0, DELAY_S] {
        let mut app = world_with_plate(Vec3::ONE, Vec3::splat(900.0));
        app.insert_resource(flat_grid(0.0));
        let shell = shot(&mut app, Vec3::Y * 2.0, Vec3::NEG_Y * SPEED, impact(delay));
        advance(&mut app);
        let (at, speed, after) = burst_mark(&app, shell);
        assert!(at.y.abs() < 1e-5, "on the surface: {at}");
        assert_eq!(speed, 0.0, "at rest");
        assert!((after - delay).abs() < 1e-6, "waits {after} s of {delay}");
    }
}

/// The game's damage model: an `Impact` fuze's delay is flight time from contact. It keeps its
/// seconds when the round slows, and no exit or bounce after contact restarts it; a force below
/// the sensitivity leaves it unarmed. The high-explosive round's delay mode relies on this to
/// burst `delay_s` past the body it struck. A metres count, which `SpendExit` keeps, bursts late
/// after the slab has slowed the round; a restart at the exit bursts late by the crossing.
#[test]
fn an_impact_delay_is_flight_time_from_contact() {
    let burst = impact(DELAY_S);
    let mut fuze = burst::Fuze::of(Some(burst));
    fuze.entered(f64::from(SENSITIVITY_N.next_down()), SPEED, None, 0.0);
    assert_eq!(fuze, burst::Fuze::Unarmed(burst), "below the sensitivity");
    fuze.entered(f64::from(SENSITIVITY_N), SPEED, None, 0.0);
    fuze.flown(0.2 * DELAY_S * SPEED);
    fuze.exited(1.0e6, 0.5 * SPEED, None);
    fuze.bounced(0.5 * SPEED, None);
    fuze.at_speed(0.5 * SPEED);
    let left = fuze.delay_left().unwrap();
    assert!((left - 0.8 * DELAY_S).abs() < 1e-6, "{left} s left");
    let before = fuze;
    fuze.entered(f64::INFINITY, SPEED, None, 0.0);
    assert_eq!(fuze, before, "an armed fuze functions once");

    let mut app = slab_of("Wood");
    let shell = shot(&mut app, ORIGIN, Vec3::NEG_Z * SPEED, burst);
    advance(&mut app);
    let (at, speed, after) = burst_mark(&app, shell);
    assert!(
        speed < 0.99 * SPEED,
        "fixture: the slab slows the round ({speed} m/s)"
    );
    assert_eq!(after, 0.0, "in flight");
    // The crossing at no less than its exit speed, then the free flight at the burst's speed.
    let exit = PLATE.z - 0.5 * THICKNESS;
    let flight_s = THICKNESS / SPEED + (exit - at.z) / speed;
    assert!(
        (flight_s - DELAY_S).abs() < 1e-3,
        "{flight_s} s of flight from contact to the burst at {at}, {speed} m/s"
    );
}

/// The game's damage model: the 8.8 cm Pzgr. 39 ring's `SpendExit` fuze keeps its outcomes.
/// Its shipped burst is the one its round file derives (113 g filler, 35.3 mm cavity), field for
/// field, and on a slab too thin to arm it, one that arms it, and one that stops it, the march
/// gives the same bursts (position, speed, wait at rest) and the same doses to the struck pool and
/// to an off-axis pool only the fragment cone reaches. The pins are MEASURED outcomes of the
/// spend-at-exit law under the shipped round data; a change to that path or to the round's burst
/// block moves them.
#[test]
fn the_spend_exit_fuze_keeps_todays_bursts_and_doses() {
    let fixture = crate::tank::WeaponRound::resolve("88mm_APHE_ring").unwrap();
    let shipped = fixture.round.burst.unwrap();
    // The ring's burst as its round file states it, DERIVED at load.
    let today = burst::Burst {
        trigger: Trigger::SpendExit { arming_spend: 35.0 },
        delay: 0.0015,
        nose: RoundBody {
            mass: 6.971_434,
            caliber: 0.088,
            nose_factor: 0.291_666_66,
        },
        sideways_speed: 534.38837,
        energy: 397_738.22,
        core_mass: shipped.core_mass,
        blast: shipped.blast,
    };
    assert_eq!(shipped, today, "the shipped burst, field for field");
    // Per slab: the bursts as (position, speed, seconds waited at rest); the pool units the shot
    // took from the struck pool and from the off-axis pool; and those the burst alone took from
    // the off-axis pool.
    type Snapshot = (Vec<([f32; 3], f32, f32)>, [u32; 2], u32);
    let pins: [Snapshot; 3] = [
        (vec![], [204_265, 13_405], 0),
        (
            vec![([0.0, 1.999_035_8, -2.914_275_2], 592.8495, 0.0)],
            [490_500, 3_675_533],
            3_643_344,
        ),
        (vec![], [1_000_000, 0], 0),
    ];
    for (thickness, pin) in [0.02, 0.05, 0.5].into_iter().zip(pins) {
        let fire = |burst| {
            let mut app = world_with_plate(Vec3::new(3.0, 3.0, thickness), PLATE);
            let world = app.world_mut();
            let entities: Vec<_> = world
                .query_filtered::<Entity, With<BallisticVolume>>()
                .iter(world)
                .collect();
            let primary = entities[0];
            for entity in entities {
                world.entity_mut(entity).insert(ComponentHealth::full(1000));
            }
            // Fixture data: an off-axis pool the round's line misses and the fragment cone
            // reaches, so its dose is the burst's potency alone.
            let side = app
                .world_mut()
                .spawn((
                    Transform::from_xyz(0.65, 2.0, -3.4),
                    avian3d::prelude::RigidBody::Static,
                    avian3d::prelude::Collider::cuboid(0.5, 1.0, 0.5),
                    avian3d::prelude::CollisionLayers::new(
                        [crate::Layer::Armor],
                        avian3d::prelude::LayerMask::ALL,
                    ),
                    BallisticVolume {
                        material: walk::VolumeMaterial::fixture(45.0),
                        substance: "fixture".into(),
                    },
                    ComponentHealth::full(100_000),
                ))
                .id();
            for _ in 0..8 {
                app.update();
            }
            let shell = shot(&mut app, ORIGIN, Vec3::NEG_Z * SPEED, burst);
            advance(&mut app);
            let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
            let positions: Vec<_> = marks
                .log
                .iter()
                .filter_map(|mark| match mark {
                    ArmourMark::Burst {
                        at, speed, after, ..
                    } => Some((at.to_array(), *speed, *after)),
                    _ => None,
                })
                .collect();
            let dose = app.world().get::<ShotDose>(shell).unwrap();
            let doses = [primary, side].map(|address| {
                dose.losses()
                    .find(|(entity, _)| *entity == address)
                    .map_or(0, |(_, loss)| loss)
            });
            let burst_dose: u32 = dose
                .settles(side)
                .iter()
                .filter(|settle| {
                    matches!(marks.log.get(settle.mark), Some(ArmourMark::Burst { .. }))
                })
                .map(|settle| settle.lost)
                .sum();
            if !positions.is_empty() {
                assert!(burst_dose > 0, "off-axis pool receives a burst dose");
            }
            (positions, doses, burst_dose)
        };
        assert_eq!(fire(shipped), pin, "a {thickness} m slab");
    }
}
