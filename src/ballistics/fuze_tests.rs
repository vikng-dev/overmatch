//! The game's damage model: the fuze in the march. An `Impact` fuze functions at contact with any
//! solid body, the ground included, and bursts its mode's delay of flight later; the `SpendExit`
//! fuze arms from resisted work at a body exit.
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
    round.flying.drag_k = 0.0;
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

/// The game's damage model: the exact free-flight apex is a transient state, never a skipped
/// step that repeats the old upward velocity forever. An armed count spends the stationary
/// tick too. FIXTURE DATA: the upward velocity cancelled by one manual gravity step.
#[test]
fn an_exact_apex_advances_the_clock_and_falls_on_the_next_step() {
    for armed in [false, true] {
        let mut app = world_with_plate(Vec3::ONE, Vec3::splat(-500.0));
        let dt = app.world().resource::<Time>().delta_secs();
        let origin = Vec3::Y * 10.0;
        let shell = shot(&mut app, origin, -gravity() * dt, impact(0.1));
        let mut round = app.world_mut().get_mut::<Projectile>(shell).unwrap();
        if armed {
            round
                .fuze
                .entered(f64::from(SENSITIVITY_N), 1000.0, None, 0.0);
        } else {
            round.fuze = burst::Fuze::Inert;
        }
        app.update();
        let round = app
            .world()
            .get::<Projectile>(shell)
            .expect("the apex round survives");
        assert_eq!(round.velocity, Vec3::ZERO);
        assert_eq!(
            app.world().get::<Transform>(shell).unwrap().translation,
            origin
        );
        if armed {
            assert!((round.fuze.delay_left().unwrap() - (0.1 - dt)).abs() < 1.0e-6);
        }
        app.update();
        assert!(app.world().get::<Projectile>(shell).unwrap().velocity.y < 0.0);
        assert!(app.world().get::<Transform>(shell).unwrap().translation.y < origin.y);
    }
}

/// The game's damage model: an armed delay that expires during the zero-displacement step
/// bursts at rest at its actual remaining wait. FIXTURE DATA: a delay shorter than one tick.
#[test]
fn a_delay_expiring_at_the_apex_bursts_without_freezing() {
    let mut app = world_with_plate(Vec3::ONE, Vec3::splat(-500.0));
    let dt = app.world().resource::<Time>().delta_secs();
    let origin = Vec3::Y * 10.0;
    let delay = dt * 0.5;
    let shell = shot(&mut app, origin, -gravity() * dt, impact(delay));
    app.world_mut()
        .get_mut::<Projectile>(shell)
        .unwrap()
        .fuze
        .entered(f64::from(SENSITIVITY_N), 1000.0, None, 0.0);
    app.update();
    assert!(app.world().get::<Projectile>(shell).is_none());
    let (at, speed, after) = burst_mark(&app, shell);
    assert_eq!(at, origin);
    assert_eq!(speed, 0.0);
    assert!((after - delay).abs() < 1.0e-6);
}

/// The game's damage model: invalid internal velocity must fail in every build, never leave a
/// round silently stuck. The fixture mutates a valid spawned round to a non-finite state.
#[test]
#[should_panic(expected = "carries finite flight velocity")]
fn nonfinite_flight_velocity_is_an_invariant_failure() {
    let mut app = world_with_plate(Vec3::ONE, Vec3::splat(-500.0));
    let shell = shot(&mut app, ORIGIN, Vec3::NEG_Z * SPEED, impact(DELAY_S));
    app.world_mut()
        .get_mut::<Projectile>(shell)
        .unwrap()
        .velocity
        .x = f32::NAN;
    app.update();
}
