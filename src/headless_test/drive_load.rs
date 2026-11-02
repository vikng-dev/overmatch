//! The game's driving and damage model, game state only: terrain observation and the drivetrain
//! response to a rearward shot. Crew are HP pools; this probe does not inspect HP outcomes.

use super::*;
use crate::bitprobe::TransmissionProbe;
use crate::track::sim::{TankTransmission, TrackDrive, TrackGripEffect};
use crate::track::transmission::{DriveSupply, price_driver_force};

struct Landing {
    gear: u8,
    price: f32,
    speed: f32,
    first_delivery: Option<f32>,
    delivery: Vec<f32>,
    terms: Vec<[f32; 4]>,
}

#[derive(Default)]
struct Measurements {
    loads: Vec<f32>,
    gaps: Vec<f32>,
    landing_speed_errors: Vec<f32>,
    pending: Option<Landing>,
    was_shifting: bool,
    last_gear: u8,
}

impl Measurements {
    fn finish_landing(&mut self) {
        let Some(landing) = self.pending.take() else {
            return;
        }; // No cut observed yet.
        if landing.delivery.is_empty() {
            return;
        } // Another cut overtook coupling settlement.
        let mean = landing.delivery.iter().map(|x| f64::from(*x)).sum::<f64>() as f32
            / landing.delivery.len() as f32;
        let gap = landing.price - mean;
        self.gaps.push(gap);
        let means: [f32; 4] = std::array::from_fn(|i| {
            landing.terms.iter().map(|t| t[i] as f64).sum::<f64>() as f32
                / landing.terms.len() as f32
        });
        println!(
            "MEASURED decomposition gear={} forecast_gap_n={gap:.6} speed_n={:.6} crank_n={:.6} inertia_n={:.6} residual_n={:.6}",
            landing.gear, means[0], means[1], means[2], means[3]
        );
        println!(
            "MEASURED sustained_landing gear={} predicted_force_n={:.6} first_delivery_n={:.6} mean_delivery_n={mean:.6} gap_n={gap:.6} samples={}",
            landing.gear,
            landing.price,
            landing
                .first_delivery
                .expect("coupled samples follow the first coupled tick"),
            landing.delivery.len()
        );
    }

    fn tick(&mut self, app: &App, tank: Entity) {
        use crate::track::transmission::ClutchState;
        let probe = probe(app, tank);
        self.loads.push(probe.perceived_load_n);
        let gear = app.world().resource::<crate::track::sim::TrackGear>();
        let state = &app.world().get::<TankTransmission>(tank).unwrap().0;
        if probe.shifting && (!self.was_shifting || state.gear != self.last_gear) {
            self.finish_landing();
            let speed = probe.shaft_speed
                - probe.perceived_load_n * f32::from(gear.trans().shift_ticks) * probe.dt
                    / gear.landing_mass();
            let price = price_driver_force(
                gear.trans(),
                DriveSupply::WHOLE,
                speed,
                probe.gear_reduction,
                1.0,
                probe.grip_limit_n,
                state.clutch,
            );
            self.pending = Some(Landing {
                gear: state.gear,
                price,
                speed,
                first_delivery: None,
                delivery: Vec::new(),
                terms: Vec::new(),
            });
        }
        if !probe.shifting
            && probe.engaged
            && let Some(landing) = &mut self.pending
        {
            assert_eq!(
                landing.gear, state.gear,
                "price and delivery must name the same landing gear"
            );
            if landing.first_delivery.is_none() {
                landing.first_delivery = Some(probe.delivered_force_n);
                self.landing_speed_errors
                    .push(probe.shaft_speed - landing.speed);
            }
            let tp = gear.trans();
            let target = match state.clutch {
                ClutchState::Out => unreachable!("an engaged probe has a coupled clutch"),
                ClutchState::Slip => tp.launch_omega(probe.throttle.abs()),
                ClutchState::In => {
                    (probe.next_speeds[0] + probe.next_speeds[1]) * 0.5 * probe.direction * probe.k
                }
            };
            // Resolution: one rpm distinguishes spool/lock transients from a settled coupling,
            // matching the public crank readout. Observe one whole interruption's coupled ticks.
            if (state.omega_e - target).abs() <= crate::track::transmission::RPM_TO_RAD {
                landing.delivery.push(probe.delivered_force_n);
                let tick_price = probe.priced_force_n;
                let crank_price =
                    (probe.tau_free.min(tp.clutch_capacity) * probe.k).min(probe.grip_limit_n);
                let acceleration = (probe.omega_integrated - probe.omega_pre) / probe.dt;
                let inertial_price = ((probe.tau_free - tp.engine_inertia * acceleration)
                    .min(tp.clutch_capacity)
                    * probe.k)
                    .min(probe.grip_limit_n);
                println!(
                    "MEASURED instantaneous gear={} forecast_n={:.6} speed_n={:.6} crank_n={:.6} inertia_n={:.6} residual_n={:.6}",
                    landing.gear,
                    landing.price - probe.delivered_force_n,
                    landing.price - tick_price,
                    tick_price - crank_price,
                    crank_price - inertial_price,
                    inertial_price - probe.delivered_force_n
                );
                landing.terms.push([
                    landing.price - tick_price,
                    tick_price - crank_price,
                    crank_price - inertial_price,
                    inertial_price - probe.delivered_force_n,
                ]);
            }
            if landing.delivery.len() >= usize::from(tp.shift_ticks).max(1) {
                self.finish_landing();
            }
        }
        self.was_shifting = probe.shifting;
        self.last_gear = state.gear;
    }

    fn report(&mut self, label: &str, expected: f32, weight: f32) {
        self.finish_landing();
        let margin = crate::track::load::DriverResolution::load().margin_n(weight);
        assert!(
            self.gaps
                .iter()
                .all(|gap| gap.is_finite() && gap.abs() < margin),
            "the measured numerical margin must cover sustained landing gaps"
        );
        let summary = |values: &[f32]| {
            if values.is_empty() {
                return String::from("none");
            }
            let mut values = values.to_vec();
            values.sort_by(f32::total_cmp);
            format!(
                "n={} min={:.6} median={:.6} p95={:.6} max={:.6}",
                values.len(),
                values[0],
                values[values.len() / 2],
                values[(values.len() - 1) * 95 / 100],
                values[values.len() - 1]
            )
        };
        println!(
            "MEASURED load {label}: weight_n={weight:.6} expected_n={expected:.6} L[{}] sustained_gap_n[{}] landing_speed_error_m_s[{}]",
            summary(&self.loads),
            summary(&self.gaps),
            summary(&self.landing_speed_errors)
        );
    }
}

fn probe(app: &App, tank: Entity) -> TransmissionProbe {
    app.world()
        .get::<TrackGripEffect>(tank)
        .unwrap()
        .transmission_probe
}

/// Fixture placement resets the complete rig, never a production spawn path. The rest height,
/// gear speeds and crank speed are construction data; pose and entry gear are fixture choices.
fn place(app: &mut BootedSim, tank: Entity, position: Vec3, rotation: Quat, speed: f32, gear: u8) {
    let mut transmission = fresh_tank_transmission(app);
    let params = app
        .world()
        .resource::<crate::track::sim::TrackGear>()
        .trans();
    transmission.0.gear = gear;
    transmission.0.omega_e = (speed * params.gears_fwd[(gear - 1) as usize]
        / params.sprocket_radius)
        .max(params.engine.idle_rpm * crate::track::transmission::RPM_TO_RAD);
    let mut e = app.world_mut().entity_mut(tank);
    {
        let mut transform = e.get_mut::<Transform>().unwrap();
        transform.translation = position;
        transform.rotation = rotation;
    }
    e.get_mut::<avian3d::prelude::Position>().unwrap().0 = position;
    e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 = rotation;
    e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 = rotation * Vec3::NEG_Z * speed;
    e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
    *e.get_mut::<TankTransmission>().unwrap() = transmission;
    *e.get_mut::<crate::track::sim::DriveSignal>().unwrap() =
        crate::track::sim::DriveSignal::default();
    for side in &mut e.get_mut::<TrackDrive>().unwrap().sides {
        side.speed = speed;
        side.phase = 0.0;
    }
    for side in &mut e
        .get_mut::<crate::track::sim::TrackGripElements>()
        .unwrap()
        .sides
    {
        side.strain.fill(Vec3::ZERO);
        side.dwell.fill(0);
    }
}

/// The Driver's load must agree with nominal weight times each declared ground's rolling
/// coefficient, independent of the scheduler's entry path. The measurement records every coupled
/// landing; no particular ground, vehicle or gear is a pass condition. Fixture: uniform flat
/// ground, rest/top-gear entry, full command for 40 simulated seconds, last-second observation.
#[test]
fn perceived_load_and_landing_gaps_on_declared_grounds() {
    for (name, ground) in crate::ground_types::GroundTypes::load_shipped().types {
        let (mut app, tank) = booted_offline_sim();
        {
            let mut track = app
                .world_mut()
                .resource_mut::<crate::track::terrain::TrackField>();
            let field = track.field.take().unwrap();
            track.field = Some(
                field.with_ground(Some(crate::ground_field::GroundField::uniform(
                    ground.contact(),
                ))),
            );
        }
        face_positive_z(&mut app, tank);
        let start = app
            .world()
            .get::<avian3d::prelude::Position>(tank)
            .unwrap()
            .0;
        let weight = app.world().resource::<TankBlueprint>().spec.mass * crate::track::derive::G;
        for rolling in [false, true] {
            let params = app
                .world()
                .resource::<crate::track::sim::TrackGear>()
                .trans();
            let (speed, gear) = if rolling {
                (params.governed_speed(false), params.gears_fwd.len() as u8)
            } else {
                (0.0, 1)
            };
            place(
                &mut app,
                tank,
                start,
                Quat::from_rotation_y(std::f32::consts::PI),
                speed,
                gear,
            );
            let mut measurements = Measurements::default();
            let mut launch_tick = None;
            let mut initial_launch_tick = None;
            // Fixture: elapsed simulation time to 2 m/s, measured on every declared ground.
            for tick in 0..40 * FIXED_TICKS_PER_SECOND {
                drive_tick(&mut app, tank, 1.0, 0.0);
                measurements.tick(&app, tank);
                let speed = app
                    .world()
                    .get::<avian3d::prelude::LinearVelocity>(tank)
                    .unwrap()
                    .0
                    .length();
                if !rolling && initial_launch_tick.is_none() && speed >= 0.5 {
                    initial_launch_tick = Some(tick + 1);
                }
                if !rolling && launch_tick.is_none() && speed >= 2.0 {
                    launch_tick = Some(tick + 1);
                }
                if tick < 39 * FIXED_TICKS_PER_SECOND {
                    measurements.loads.clear();
                }
            }
            if !rolling {
                println!(
                    "MEASURED launch {name}: time_to_0_5_m_s={:?} time_to_2_m_s={:?}",
                    initial_launch_tick.map(|ticks| ticks as f32 / FIXED_TICKS_PER_SECOND as f32),
                    launch_tick.map(|ticks| ticks as f32 / FIXED_TICKS_PER_SECOND as f32)
                );
            }
            let expected = ground.contact().rolling_resistance * weight;
            measurements.report(&format!("{name} rolling={rolling}"), expected, weight);
            for load in measurements.loads {
                assert!(
                    (load - expected).abs() < weight * 1e-4,
                    "a uniform plane's observation must match its own coefficient: {load} vs {expected}"
                );
            }
        }
        let mut turned = Vec::new();
        for _ in 0..128 {
            drive_tick(&mut app, tank, 1.0, 0.7);
            turned.push(probe(&app, tank).perceived_load_n);
        }
        let expected = ground.contact().rolling_resistance * weight;
        let extra = turned.iter().copied().fold(f32::NEG_INFINITY, f32::max) - expected;
        println!("MEASURED turn {name}: peak load above straight {extra:.3} N");
        assert!(
            extra > 0.0,
            "a finite-radius rolling turn must charge resistance"
        );
    }
}

/// The course's support normals, rather than hull acceleration or suspension force, must set
/// L. The gear-law calibration relies on that invariant on an incline as well as on the flat.
/// Fixture: uninterrupted 10/20/30 degree ramp planes, parked then a rolling entry.
/// Extending the faces isolates their support from the flat slab and short course crests.
#[test]
fn perceived_load_and_landing_gaps_on_the_course() {
    for degrees in [10.0_f32, 20.0, 30.0] {
        let angle = degrees.to_radians();
        // Fixture: one plane over the synthetic world's extent, rising toward -Z.
        let extent = crate::terrain_grid::FIXTURE_EXTENT;
        let height = extent.half_extent() * angle.tan();
        let grid = crate::terrain_grid::HeightGrid::new(
            vec![height, height, -height, -height].into(),
            2,
            extent,
        );
        let mut app = booted_sim_on(Some(grid));
        start_fixed_clock(&mut app);
        app.update(); // Initialize the physics body before the fixture teleports it.
        let tank = app
            .world_mut()
            .query_filtered::<Entity, (With<Tank>, With<Controlled>)>()
            .single(app.world())
            .expect("one controlled tank");
        let rotation = Quat::from_rotation_x(angle);
        let normal = rotation * Vec3::Y;
        let rest = app
            .world()
            .resource::<crate::track::rig_geom::RigGeom>()
            .hull_rest_y;
        let position = Vec3::new(100.0, 0.0, 0.0) + normal * rest;
        place(&mut app, tank, position, rotation, 0.0, 1);
        for _ in 0..128 {
            drive_tick(&mut app, tank, 0.0, 0.0);
        }
        let weight = app.world().resource::<TankBlueprint>().spec.mass * crate::track::derive::G;
        let rolling = crate::ground_types::GroundTypes::load_shipped()
            .get(crate::terrain_blend::BASE_GROUND)
            .contact()
            .rolling_resistance;
        let expected = weight * (angle.sin() + rolling * angle.cos());
        let mut measurements = Measurements::default();
        for _ in 0..64 {
            drive_tick(&mut app, tank, 0.0, 0.0);
            measurements.tick(&app, tank);
            let load = probe(&app, tank).perceived_load_n;
            assert!(
                (load - expected).abs() < weight * 1e-4,
                "a supported incline must read its own geometry: {degrees} degrees, {load} vs {expected}; position {:?}, contacts {}",
                app.world()
                    .get::<avian3d::prelude::Position>(tank)
                    .unwrap()
                    .0,
                app.world()
                    .get::<crate::track::sim::TrackContacts>(tank)
                    .unwrap()
                    .0
                    .iter()
                    .map(Vec::len)
                    .sum::<usize>()
            );
        }
        // Fixture: excite the existing scheduler with a rolling entry, without replacing it.
        let position = app
            .world()
            .get::<avian3d::prelude::Position>(tank)
            .unwrap()
            .0;
        place(&mut app, tank, position, rotation, 3.0, 6);
        for _ in 0..512 {
            drive_tick(&mut app, tank, 1.0, 0.0);
            measurements.tick(&app, tank);
            let load = probe(&app, tank).perceived_load_n;
            assert!(
                (load - expected).abs() < weight * 1e-4,
                "rolling support must price its incline independently of acceleration: {load} vs {expected}"
            );
        }
        measurements.report(&format!("course {degrees} degrees"), expected, weight);
    }
}

/// The game's driving and damage model: a rearward shot may move the hull and its contact
/// forces, but must not become a terrain-load step. The next gear law relies on that separation.
/// Fixture: part command on the course's middle incline, a rear-facing primary weapon, paired
/// deterministic runs with and without one shot. HP outcomes are outside this probe.
#[test]
fn perceived_load_during_a_rearward_shot_uphill() {
    let run = |fire: bool| {
        let angle = 20.0_f32.to_radians();
        // Fixture: an uninterrupted incline; recoil cannot move a patch across a ramp edge.
        let extent = crate::terrain_grid::FIXTURE_EXTENT;
        let height = extent.half_extent() * angle.tan();
        let grid = crate::terrain_grid::HeightGrid::new(
            vec![height, height, -height, -height].into(),
            2,
            extent,
        );
        let mut app = booted_sim_on(Some(grid));
        start_fixed_clock(&mut app);
        app.update();
        let tank = app
            .world_mut()
            .query_filtered::<Entity, (With<Tank>, With<Controlled>)>()
            .single(app.world())
            .expect("the fixture spawns one controlled tank");
        let rotation = Quat::from_rotation_x(angle);
        let normal = rotation * Vec3::Y;
        let rest = app
            .world()
            .resource::<crate::track::rig_geom::RigGeom>()
            .hull_rest_y;
        let position = Vec3::new(100.0, 0.0, 0.0) + normal * rest;
        place(&mut app, tank, position, rotation, 0.0, 1);
        for _ in 0..128 {
            drive_tick(&mut app, tank, 0.0, 0.0);
        }
        let slots: Vec<_> = app
            .world_mut()
            .query::<(
                &crate::tank::TankRoot,
                &crate::tank::ServoIndex,
                &crate::tank::ServoRole,
            )>()
            .iter(app.world())
            .filter_map(|(root, index, role)| {
                (root.0 == tank && matches!(role, crate::tank::ServoRole::Yaw)).then_some(index.0)
            })
            .collect();
        assert!(!slots.is_empty(), "fixture has a yawing weapon mount");
        for slot in slots {
            app.world_mut()
                .get_mut::<crate::tank::TankServos>(tank)
                .unwrap()
                .states[slot] =
                crate::tank::ServoState::test_new(std::f32::consts::PI, std::f32::consts::PI, 0.0);
        }
        app.world_mut().get_mut::<TankCommand>(tank).unwrap().aim =
            Some(crate::command::AimIntent::HullLocal(Vec3::Z * 1000.0));
        let mut samples = Vec::new();
        for tick in 0..128 {
            app.world_mut()
                .get_mut::<TankCommand>(tank)
                .unwrap()
                .fire_primary = fire && tick == 64;
            drive_tick(&mut app, tank, 0.4, 0.0);
            let load = probe(&app, tank).perceived_load_n;
            let weight =
                app.world().resource::<TankBlueprint>().spec.mass * crate::track::derive::G;
            let rolling = crate::ground_types::GroundTypes::load_shipped()
                .get(crate::terrain_blend::BASE_GROUND)
                .contact()
                .rolling_resistance;
            let expected = weight * (angle.sin() + rolling * angle.cos());
            assert!(
                (load - expected).abs() < weight * 1e-4,
                "the shot fixture must remain on its declared incline: {load} vs {expected}"
            );
            samples.push(load);
        }
        let fired = app
            .world()
            .get::<crate::tank::TankSim>(tank)
            .unwrap()
            .weapons
            .iter()
            .map(|w| w.rounds_fired)
            .sum::<u32>();
        assert_eq!(
            fired > 0,
            fire,
            "the paired fixture must actually distinguish a shot"
        );
        (
            samples,
            app.world().resource::<TankBlueprint>().spec.mass * crate::track::derive::G,
        )
    };
    let (baseline, weight) = run(false);
    let (fired, _) = run(true);
    let change = baseline
        .iter()
        .zip(fired)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    println!("MEASURED rearward shot uphill: maximum paired L change {change:.6} N");
    // Resolution: the fixed support plane must stay within one ten-thousandth of nominal
    // weight; a force-calibration margin must never mask recoil entering the terrain observer.
    let margin = weight * 1e-4;
    assert!(
        change < margin,
        "recoil entered the terrain observation: {change} N"
    );
}

/// The game's driving and damage model: crew are HP pools at stations. The shipped sim must
/// retain the empty station's existing park/neutral at rest, and re-engage a moving clutch after
/// every interruption without idle-catch work. Fixture: a uniform 10-degree descent, then 5 m/s.
#[test]
fn empty_driver_station_rest_and_descent() {
    use crate::damage::{CrewStation, Dead, VolumeOf};
    use crate::track::transmission::{ClutchState, PARK_ENGAGE_SPEED, RPM_TO_RAD};
    let angle = -10.0_f32.to_radians(); // Fixture descent toward the forward axis.
    let extent = crate::terrain_grid::FIXTURE_EXTENT;
    let height = extent.half_extent() * angle.tan();
    let grid = crate::terrain_grid::HeightGrid::new(
        vec![height, height, -height, -height].into(),
        2,
        extent,
    );
    let mut app = booted_sim_on(Some(grid));
    start_fixed_clock(&mut app);
    app.update();
    let tank = app
        .world_mut()
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>()
        .single(app.world())
        .expect("one controlled fixture tank");
    let rotation = Quat::from_rotation_x(angle);
    let normal = rotation * Vec3::Y;
    let rest = app
        .world()
        .resource::<crate::track::rig_geom::RigGeom>()
        .hull_rest_y;
    let position = Vec3::new(100.0, 0.0, 0.0) + normal * rest;
    place(&mut app, tank, position, rotation, 0.0, 1);
    for _ in 0..128 {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    // Empty every pool assigned to this station before the next simulation tick.
    let drivers: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &CrewStation, &VolumeOf)>()
        .iter(app.world())
        .filter_map(|(entity, station, owner)| {
            (*station == CrewStation::Driver && owner.0 == tank).then_some(entity)
        })
        .collect();
    assert!(!drivers.is_empty(), "the fixture has a Driver station");
    for driver in drivers {
        app.world_mut()
            .get_mut::<crate::ballistics::ComponentHealth>(driver)
            .unwrap()
            .set_held(0);
        app.world_mut().entity_mut(driver).insert(Dead);
    }
    let start = app
        .world()
        .get::<avian3d::prelude::Position>(tank)
        .unwrap()
        .0;
    let mut previous_crank = app.world().get::<TankTransmission>(tank).unwrap().0.omega_e;
    for _ in 0..4 * FIXED_TICKS_PER_SECOND {
        drive_tick(&mut app, tank, 0.0, 0.0);
        let state = &app.world().get::<TankTransmission>(tank).unwrap().0;
        assert!(state.park);
        assert_eq!(state.clutch, ClutchState::Out);
        assert!(
            state.omega_e <= previous_crank,
            "a disconnected crank without fuel cannot gain energy"
        );
        previous_crank = state.omega_e;
        assert_eq!(probe(&app, tank).tau_idle, 0.0);
    }
    let state = &app.world().get::<TankTransmission>(tank).unwrap().0;
    let end = app
        .world()
        .get::<avian3d::prelude::Position>(tank)
        .unwrap()
        .0;
    println!(
        "MEASURED empty Driver at rest on 10 degrees after 4 s: park={}, clutch={:?}, rpm={}, displacement_m={}",
        state.park,
        state.clutch,
        state.omega_e / RPM_TO_RAD,
        (end - start).length()
    );
    // Fixture: a rolling entry in the upper-middle portion of the authored gear ladder.
    let gear = (app
        .world()
        .resource::<crate::track::sim::TrackGear>()
        .trans()
        .gears_fwd
        .len()
        * 3
        / 4)
    .max(1) as u8;
    place(&mut app, tank, position, rotation, 5.0, gear);
    app.world_mut()
        .get_mut::<TankTransmission>(tank)
        .unwrap()
        .0
        .clutch = ClutchState::In;
    let mut interrupted_ticks = 0;
    let mut first_drag = None;
    for _ in 0..8 * FIXED_TICKS_PER_SECOND {
        drive_tick(&mut app, tank, 0.0, 0.0);
        let sample = probe(&app, tank);
        let state = &app.world().get::<TankTransmission>(tank).unwrap().0;
        assert_eq!(sample.tau_idle, 0.0);
        assert_eq!(sample.tau_induced, 0.0);
        if sample.shifting {
            interrupted_ticks += 1;
        } else if sample.mean_speed.abs() >= PARK_ENGAGE_SPEED {
            assert_eq!(
                state.clutch,
                ClutchState::In,
                "a moving interruption must expire even at an empty station"
            );
            if first_drag.is_none() {
                first_drag = Some(sample.forces.iter().sum::<f32>());
            }
        }
    }
    assert!(
        first_drag.unwrap() < 0.0,
        "the rolling entry must receive motoring drag"
    );
    let speed = app
        .world()
        .get::<avian3d::prelude::LinearVelocity>(tank)
        .unwrap()
        .0
        .dot(rotation * Vec3::NEG_Z);
    let state = &app.world().get::<TankTransmission>(tank).unwrap().0;
    println!(
        "MEASURED empty Driver entering at 5 m/s on 10 degrees after 8 s: first_motor_force_n={}, speed_m_s={speed}, gear={}, clutch={:?}, rpm={}, interrupted_ticks={interrupted_ticks}",
        first_drag.unwrap(),
        state.gear,
        state.clutch,
        state.omega_e / RPM_TO_RAD
    );
}
