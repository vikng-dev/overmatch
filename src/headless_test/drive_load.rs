//! The game's driving and damage model, game state only: terrain observation and the drivetrain
//! response to a rearward shot. Crew are HP pools; this probe does not inspect HP outcomes.

use super::*;
use crate::bitprobe::TransmissionProbe;
use crate::track::sim::{TankTransmission, TrackDrive, TrackGripEffect};
use crate::track::transmission::{DriveSupply, price_driver_force};

#[derive(Default)]
struct Measurements {
    loads: Vec<f32>,
    gaps: Vec<f32>,
    landing_speed_errors: Vec<f32>,
    pending: Option<(u8, f32, f32)>,
    was_shifting: bool,
    last_gear: u8,
}

impl Measurements {
    fn tick(&mut self, app: &App, tank: Entity) {
        let probe = probe(app, tank);
        self.loads.push(probe.perceived_load_n);
        let gear = app.world().resource::<crate::track::sim::TrackGear>();
        let current_gear = app.world().get::<TankTransmission>(tank).unwrap().0.gear;
        if probe.shifting && (!self.was_shifting || current_gear != self.last_gear) {
            let landed_speed = probe.shaft_speed
                - probe.perceived_load_n * f32::from(gear.trans().shift_ticks) * probe.dt
                    / gear.landing_mass();
            let price = price_driver_force(
                gear.trans(),
                DriveSupply::WHOLE,
                landed_speed,
                probe.gear_reduction,
                1.0,
                probe.grip_limit_n,
            );
            self.pending = Some((
                app.world().get::<TankTransmission>(tank).unwrap().0.gear,
                price,
                landed_speed,
            ));
        }
        if !probe.shifting
            && probe.engaged
            && let Some((landed_gear, price, speed)) = self.pending.take()
        {
            assert_eq!(
                landed_gear, current_gear,
                "a price must be compared with delivery in the same landing gear"
            );
            let gap = price - probe.delivered_force_n;
            self.gaps.push(gap);
            self.landing_speed_errors.push(probe.shaft_speed - speed);
            println!(
                "MEASURED landing gear={landed_gear} predicted_force_n={price:.3} delivered_force_n={:.3} gap_n={gap:.3} price_at_actual_n={:.3} speed_error_m_s={:.6}",
                probe.delivered_force_n,
                probe.priced_force_n,
                probe.shaft_speed - speed
            );
        }
        self.was_shifting = probe.shifting;
        self.last_gear = current_gear;
    }

    fn report(&self, label: &str, expected: f32) {
        let margin = crate::track::load::DriverResolution::load().landing_force_margin_n;
        assert!(
            self.gaps
                .iter()
                .all(|gap| gap.is_finite() && gap.abs() < margin),
            "the measured numerical margin must cover every observed landing gap"
        );
        let summary = |values: &[f32]| {
            if values.is_empty() {
                return String::from("none");
            }
            let mut values = values.to_vec();
            values.sort_by(f32::total_cmp);
            format!(
                "n={} min={:.3} median={:.3} p95={:.3} max={:.3}",
                values.len(),
                values[0],
                values[values.len() / 2],
                values[(values.len() - 1) * 95 / 100],
                values[values.len() - 1]
            )
        };
        println!(
            "MEASURED load {label}: expected_n={expected:.3} L[{}] landing_gap_n[{}] landing_speed_error_m_s[{}]",
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
            for tick in 0..40 * FIXED_TICKS_PER_SECOND {
                drive_tick(&mut app, tank, 1.0, 0.0);
                measurements.tick(&app, tank);
                if tick < 39 * FIXED_TICKS_PER_SECOND {
                    measurements.loads.clear();
                }
            }
            let expected = ground.contact().rolling_resistance * weight;
            measurements.report(&format!("{name} rolling={rolling}"), expected);
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
        }
        measurements.report(&format!("course {degrees} degrees"), expected);
    }
}

/// The game's driving and damage model: a rearward shot may move the hull and its contact
/// forces, but must not become a terrain-load step. The next gear law relies on that separation.
/// Fixture: part command on the course's middle incline, a rear-facing primary weapon, paired
/// deterministic runs with and without one shot. HP outcomes are outside this probe.
#[test]
fn perceived_load_during_a_rearward_shot_uphill() {
    let run = |fire: bool| {
        let (mut app, tank) = booted_offline_sim();
        let angle = 20.0_f32.to_radians();
        let rotation = Quat::from_rotation_x(angle);
        let normal = rotation * Vec3::Y;
        let rest = app
            .world()
            .resource::<crate::track::rig_geom::RigGeom>()
            .hull_rest_y;
        let position = Vec3::new(0.0, -1.0 + 5.0 * angle.sin(), -40.0) + normal * rest;
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
            samples.push(probe(&app, tank).perceived_load_n);
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
        samples
    };
    let baseline = run(false);
    let fired = run(true);
    let change = baseline
        .iter()
        .zip(fired)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    println!("MEASURED rearward shot uphill: maximum paired L change {change:.6} N");
    // The gear law relies on recoil changing L by less than its admitted force resolution.
    let margin = crate::track::load::DriverResolution::load().landing_force_margin_n;
    assert!(
        change < margin,
        "recoil entered the terrain observation: {change} N"
    );
}
