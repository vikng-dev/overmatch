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
    residuals: Vec<f32>,
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
        if self
            .pending
            .as_ref()
            .is_some_and(|landing| landing.gear != state.gear)
        {
            // A parked C3 preselection pays no cut and is not a landing in the previous gear.
            self.finish_landing();
        }
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
                self.residuals
                    .push(inertial_price - probe.delivered_force_n);
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

    fn report(
        &mut self,
        label: &str,
        expected: f32,
        weight: f32,
        tp: &crate::track::transmission::TransmissionParams,
    ) {
        self.finish_landing();
        let margin = crate::track::load::DriverResolution::load().margin_n(weight, tp);
        assert!(
            self.residuals
                .iter()
                .all(|gap| gap.is_finite() && gap.abs() < margin),
            "the measured deadband must cover the same-instant numerical residual"
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
pub(super) fn place(
    app: &mut BootedSim,
    tank: Entity,
    position: Vec3,
    rotation: Quat,
    speed: f32,
    gear: u8,
) {
    let mut transmission = fresh_tank_transmission(app);
    let params = app
        .world()
        .resource::<crate::track::sim::TrackGear>()
        .trans();
    transmission.0.gear = gear;
    transmission.0.park = speed.abs() < crate::track::transmission::PARK_ENGAGE_SPEED;
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
/// coefficient, independent of the Driver's entry path. The measurement records every coupled
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
            measurements.report(
                &format!("{name} rolling={rolling}"),
                expected,
                weight,
                app.world()
                    .resource::<crate::track::sim::TrackGear>()
                    .trans(),
            );
            for load in measurements.loads {
                assert!(
                    (load - expected).abs() < weight * 1e-4,
                    "a uniform plane's observation must match its own coefficient: {load} vs {expected}"
                );
            }
        }
        // Fixture: frozen supported pose on the uniform plane. The production belt tick returns
        // its geometric patches, so this expectation includes finite patch spans independently
        // of observe's own integral and catches a missing turn radius at the adapter seam.
        let construction = app.world().resource::<crate::track::sim::TrackGear>();
        let blueprint = app.world().resource::<TankBlueprint>();
        for steer in [0.25_f32, 1.0] {
            let mut drive = TrackDrive::default();
            let mut signal = crate::track::sim::DriveSignal {
                throttle: 1.0,
                steer,
            };
            let mut grip = crate::track::sim::TrackGrip::default();
            let mut elements =
                crate::track::sim::TrackGripElements::for_links(blueprint.spec.track.link_count);
            let mut state =
                crate::track::transmission::TransmissionState::from_spec(construction.trans());
            state.gear = construction
                .trans()
                .launch_gears
                .min(construction.trans().gears_fwd.len() as u8);
            state.shift_ticks = 1; // Isolate the observer from a new gear decision.
            state.steer_step = if steer < 0.5 { 1 } else { 2 };
            let radius = crate::track::load::turn_radius(construction.trans(), &state).unwrap();
            let (reports, report) = crate::track::sim::belt_tick(
                construction,
                app.world()
                    .resource::<crate::track::terrain::TrackField>()
                    .field
                    .as_ref()
                    .unwrap(),
                &crate::track::rubble::RubbleBodies::default(),
                crate::track::drive::DriveTarget {
                    intent: crate::track::drive::DriveIntent::Axes(
                        crate::track::drive::DriveAxes {
                            throttle: 1.0,
                            steer,
                        },
                    ),
                    supply: DriveSupply::WHOLE,
                },
                bevy::math::Affine3A::from_rotation_translation(
                    Quat::from_rotation_y(std::f32::consts::PI),
                    start,
                ),
                1.0 / FIXED_TICKS_PER_SECOND as f32,
                |_| Vec3::ZERO,
                crate::track::sim::BeltRig {
                    drive: &mut drive,
                    signal: &mut signal,
                    grip: &mut grip,
                    elements: &mut elements,
                    transmission: &mut state,
                },
            );
            let facts: Vec<_> = reports
                .iter()
                .flat_map(|r| r.perceived_support.iter())
                .collect();
            assert!(
                !facts.is_empty(),
                "the frozen turn fixture must bear on its plane"
            );
            let lo = facts
                .iter()
                .map(|c| c.point.z - c.half_span.z.abs())
                .fold(f32::INFINITY, f32::min);
            let hi = facts
                .iter()
                .map(|c| c.point.z + c.half_span.z.abs())
                .fold(f32::NEG_INFINITY, f32::max);
            let mu = report.bitprobe.grip_limit_n / weight;
            let expected_turn = mu * weight * (hi - lo) / (4.0 * radius);
            let actual_turn =
                report.bitprobe.perceived_load_n - ground.contact().rolling_resistance * weight;
            assert!(
                (actual_turn - expected_turn).abs() < weight * 1e-5,
                "the shipped turn must charge its full uniform-pressure moment: {actual_turn} vs {expected_turn}"
            );
        }
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
        // Observation on a supported plane does not imply a resting body. The launch proof
        // starts with fresh motion, strain and drivetrain state on that same supported pose.
        let launch_position = app
            .world()
            .get::<avian3d::prelude::Position>(tank)
            .unwrap()
            .0;
        place(&mut app, tank, launch_position, rotation, 0.0, 1);
        app.world_mut()
            .get_mut::<TankTransmission>(tank)
            .unwrap()
            .0
            .park = true;
        assert_eq!(
            app.world()
                .get::<avian3d::prelude::LinearVelocity>(tank)
                .unwrap()
                .0,
            Vec3::ZERO
        );
        assert!(
            app.world()
                .get::<TrackDrive>(tank)
                .unwrap()
                .sides
                .iter()
                .all(|side| side.speed == 0.0)
        );
        let mut launch_gear = 0;
        let mut cuts = 0;
        let mut was_cut = false;
        let mut minimum = 0.0_f32;
        for tick in 0..30 * FIXED_TICKS_PER_SECOND {
            drive_tick(&mut app, tank, 1.0, 0.0);
            if tick == 0 {
                launch_gear = app.world().get::<TankTransmission>(tank).unwrap().0.gear;
            }
            let p = probe(&app, tank);
            cuts += usize::from(p.shifting && !was_cut);
            was_cut = p.shifting;
            minimum = minimum.min(p.next_speeds[0]).min(p.next_speeds[1]);
            if degrees == 20.0 {
                // Fixture: S2's specified class witness.
                assert!(
                    p.next_speeds
                        .iter()
                        .all(|v| *v >= -crate::track::transmission::PARK_ENGAGE_SPEED),
                    "a supported launch must not roll against intent: tick={tick} speeds={:?} next={:?} forces={:?}",
                    p.speeds,
                    p.next_speeds,
                    p.forces
                );
            }
        }
        let climb = (app
            .world()
            .get::<avian3d::prelude::Position>(tank)
            .unwrap()
            .0
            - launch_position)
            .dot(rotation * Vec3::NEG_Z);
        println!(
            "MEASURED grade launch degrees={degrees} gear={launch_gear} distance_m={climb} cuts={cuts} minimum_belt_m_s={minimum}"
        );
        if degrees == 20.0 {
            assert!(cuts <= 1, "S2 must not cascade through cuts");
        }
        assert!(
            climb > 0.0,
            "a supported capable launch must gain distance uphill"
        );
        // Fixture: observe a rolling entry through the same supported grade.
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
        measurements.report(
            &format!("course {degrees} degrees"),
            expected,
            weight,
            app.world()
                .resource::<crate::track::sim::TrackGear>()
                .trans(),
        );
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
        let mut gears = Vec::new();
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
            let state = app.world().get::<TankTransmission>(tank).unwrap().0;
            gears.push((state.gear, state.shift_ticks));
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
            gears,
            app.world().resource::<TankBlueprint>().spec.mass * crate::track::derive::G,
        )
    };
    let (baseline, baseline_gears, weight) = run(false);
    let (fired, fired_gears, _) = run(true);
    assert_eq!(
        baseline_gears, fired_gears,
        "a shot must not change the gear decision or interruption timeline"
    );
    assert!(
        fired_gears[64..]
            .iter()
            .all(|sample| sample.0 == fired_gears[63].0),
        "the shot must not start a shift"
    );
    println!(
        "MEASURED rearward shot uphill: gear {} held through the shot",
        fired_gears[63].0
    );
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

/// The game's driving and damage model: crew are HP pools at stations. Without the Driver,
/// the shipped clutch must stay engaged through an uphill turning point as well as a descent.
/// Construction's drag anchor predicts a terminal speed where its rising torque balances
/// gravity minus rolling resistance below the crank guard. A weaker-drag mutant must miss that
/// equilibrium; a crank guard alone is not a belt-speed bound in taller gears.
/// Fixture: fresh 10-degree plane per entry, 5 m/s in launch and upper-middle gears; 40 seconds in the launch gear and 20 in the upper-middle gear.
#[test]
fn empty_driver_station_coast_and_rollback() {
    use crate::damage::{CrewStation, Dead, VolumeOf};
    use crate::track::transmission::{ClutchState, OVERREV_MARGIN_RPM, RPM_TO_RAD};
    for degrees in [-10.0_f32, 10.0] {
        for launch_gear in [true, false] {
            let angle = degrees.to_radians();
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
                .unwrap();
            let rotation = Quat::from_rotation_x(angle);
            let rest = app
                .world()
                .resource::<crate::track::rig_geom::RigGeom>()
                .hull_rest_y;
            let position = Vec3::new(100.0, 0.0, 0.0) + rotation * Vec3::Y * rest;
            let tp = app
                .world()
                .resource::<crate::track::sim::TrackGear>()
                .trans();
            let gear = if launch_gear {
                tp.launch_gears
            } else {
                (tp.gears_fwd.len() * 3 / 4).max(1) as u8
            };
            let guard = (tp.max_curve_rpm() + OVERREV_MARGIN_RPM) * RPM_TO_RAD;
            // DERIVED equilibrium from authored facts, independent of engine_drag's implementation:
            // k*tau = W*(|sin(theta)| - c_rr*cos(theta)), tau = anchor*(b+(1-b)*omega/mid).
            let k = tp.gears_fwd[usize::from(gear - 1)] / tp.sprocket_radius;
            let weight =
                app.world().resource::<TankBlueprint>().spec.mass * crate::track::derive::G;
            let rolling = crate::ground_types::GroundTypes::load_shipped()
                .get(crate::terrain_blend::BASE_GROUND)
                .contact()
                .rolling_resistance;
            let torque = weight * (angle.sin().abs() - rolling * angle.cos()) / k;
            let base = crate::track::transmission::motoring_drag_base_share();
            let mid = (tp.engine.idle_rpm + tp.engine.governed_rpm) * 0.5 * RPM_TO_RAD;
            let omega =
                mid * (torque / (tp.peak_torque_nm * tp.drag_fraction) - base) / (1.0 - base);
            // This affine inverse is outside the one-rpm rest regularization and below both caps.
            let equilibrium = (omega > RPM_TO_RAD && omega < guard && torque < tp.clutch_capacity)
                .then_some(omega / k);
            place(&mut app, tank, position, rotation, 5.0, gear);
            app.world_mut()
                .get_mut::<TankTransmission>(tank)
                .unwrap()
                .0
                .clutch = ClutchState::In;
            let drivers: Vec<_> = app
                .world_mut()
                .query::<(Entity, &CrewStation, &VolumeOf)>()
                .iter(app.world())
                .filter_map(|(e, station, owner)| {
                    (*station == CrewStation::Driver && owner.0 == tank).then_some(e)
                })
                .collect();
            assert!(!drivers.is_empty());
            for driver in drivers {
                app.world_mut()
                    .get_mut::<crate::ballistics::ComponentHealth>(driver)
                    .unwrap()
                    .set_held(0);
                app.world_mut().entity_mut(driver).insert(Dead);
            }
            let mut min_speed = 5.0_f32;
            let mut max_speed = 5.0_f32;
            let mut final_lo = f32::INFINITY;
            let mut final_hi = f32::NEG_INFINITY;
            let mut final_drag = 0.0;
            let mut max_rpm = 0.0_f32;
            let seconds = if launch_gear { 40 } else { 20 };
            for tick in 0..seconds * FIXED_TICKS_PER_SECOND {
                drive_tick(&mut app, tank, 0.0, 0.0);
                let sample = probe(&app, tank);
                let state = &app.world().get::<TankTransmission>(tank).unwrap().0;
                assert_eq!(
                    state.clutch,
                    ClutchState::In,
                    "no Driver can open the clutch"
                );
                assert_eq!(state.gear, gear);
                assert!(!sample.shifting && !state.park);
                assert_eq!([sample.tau_induced, sample.tau_idle], [0.0; 2]);
                assert!(state.omega_e.abs() <= guard + 0.001);
                assert!(sample.grip_limit_n > 0.0);
                let speed = app
                    .world()
                    .get::<avian3d::prelude::LinearVelocity>(tank)
                    .unwrap()
                    .0
                    .dot(rotation * Vec3::NEG_Z);
                min_speed = min_speed.min(speed);
                max_speed = max_speed.max(speed);
                max_rpm = max_rpm.max(state.omega_e.abs() / RPM_TO_RAD);
                if tick >= (seconds - 1) * FIXED_TICKS_PER_SECOND {
                    final_lo = final_lo.min(speed);
                    final_hi = final_hi.max(speed);
                    final_drag += sample.delivered_force_n;
                }
            }
            let state = &app.world().get::<TankTransmission>(tank).unwrap().0;
            assert_eq!(
                state.omega_e.signum(),
                if degrees > 0.0 { -1.0 } else { 1.0 },
                "uphill coast must roll back through the crank"
            );
            let drag = final_drag / FIXED_TICKS_PER_SECOND as f32;
            assert!(
                if degrees > 0.0 {
                    drag > 0.0
                } else {
                    drag < 0.0
                },
                "steady motoring drag must oppose travel"
            );
            if let Some(expected) = equilibrium {
                // Resolution: one percent permits the coupled contact discretization and finite
                // settling window; halving the authored drag's delivery cannot fit this band.
                let precision = expected * 0.01;
                assert!(
                    (final_lo.abs() - expected).abs() < precision
                        && (final_hi.abs() - expected).abs() < precision,
                    "motoring equilibrium {expected} m/s, measured [{final_lo}, {final_hi}]"
                );
                assert!(
                    final_hi - final_lo < precision,
                    "motoring equilibrium must settle"
                );
            }
            println!(
                "MEASURED S3 grade_deg={degrees} gear={gear} min_speed_m_s={min_speed:.6} max_speed_m_s={max_speed:.6} final_range_m_s=[{final_lo:.6},{final_hi:.6}] max_abs_rpm={max_rpm:.6} mean_final_drag_n={drag:.6} expected_equilibrium_m_s={equilibrium:?} speed_settled={}",
                final_hi - final_lo < 0.01
            );
        }
    }
}

/// The shipped composition must launch at every player notch on level ground and on a
/// supported grade, and sustain increasing steady travel. Fixture: a long firm plane.
#[test]
fn shipped_notches_launch_and_sustain_ordered_speed() {
    for degrees in [0.0_f32, 10.0] {
        let mut last = 0.0;
        let step = crate::track::drive::drive_feel().notch_step.0;
        for pedal in std::iter::successors(Some(step), |pedal| {
            (*pedal < 1.0).then(|| (*pedal + step).min(1.0))
        }) {
            let angle = degrees.to_radians();
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
                .unwrap();
            let rest = app
                .world()
                .resource::<crate::track::rig_geom::RigGeom>()
                .hull_rest_y;
            // Fixture: the plane rises toward -Z; travel climbs it from the positive end.
            let rotation = if degrees == 0.0 {
                Quat::from_rotation_y(std::f32::consts::PI)
            } else {
                Quat::from_rotation_x(angle)
            };
            let z = if degrees == 0.0 { -350.0 } else { 350.0 };
            let position = Vec3::new(100.0, -z * angle.tan(), z) + rotation * Vec3::Y * rest;
            place(&mut app, tank, position, rotation, 0.0, 1);
            for _ in 0..128 {
                drive_tick(&mut app, tank, 0.0, 0.0);
            }
            let position = app
                .world()
                .get::<avian3d::prelude::Position>(tank)
                .unwrap()
                .0;
            place(&mut app, tank, position, rotation, 0.0, 1);
            use bevy::ecs::system::RunSystemOnce;
            let supply = app
                .world_mut()
                .run_system_once(
                    move |tanks: Query<(
                        &crate::damage::TankVolumes,
                        &crate::damage::TankCapabilities,
                    )>,
                          facets: Query<crate::damage::VolumeFacets>| {
                        let (volumes, caps) = tanks.get(tank).unwrap();
                        let request = crate::command::ParsedTankCommand::try_from(TankCommand {
                            throttle: pedal,
                            ..default()
                        })
                        .unwrap();
                        crate::track::sim::drive_target(
                            &request,
                            Some(volumes),
                            Some(caps),
                            &facets,
                        )
                        .supply
                    },
                )
                .unwrap();
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            let mut sum = 0.0;
            let mut capable = false;
            for tick in 0..60 * FIXED_TICKS_PER_SECOND {
                drive_tick(&mut app, tank, pedal, 0.0);
                let p = probe(&app, tank);
                if tick == 0 {
                    let tp = app
                        .world()
                        .resource::<crate::track::sim::TrackGear>()
                        .trans();
                    capable = (1..=tp.launch_gears).any(|g| {
                        price_driver_force(
                            tp,
                            supply,
                            0.0,
                            tp.gears_fwd[usize::from(g - 1)],
                            pedal,
                            p.grip_limit_n,
                            crate::track::transmission::ClutchState::Slip,
                        ) > p.perceived_load_n
                    });
                }
                assert!(
                    p.grip_limit_n > 0.0,
                    "the notch proof must remain supported"
                );
                assert!(
                    p.next_speeds
                        .iter()
                        .all(|v| *v >= -crate::track::transmission::PARK_ENGAGE_SPEED),
                    "grade={degrees} pedal={pedal} tick={tick} speeds={:?} next={:?} forces={:?}",
                    p.speeds,
                    p.next_speeds,
                    p.forces
                );
                if tick >= 59 * FIXED_TICKS_PER_SECOND {
                    let v = app
                        .world()
                        .get::<avian3d::prelude::LinearVelocity>(tank)
                        .unwrap()
                        .0
                        .dot(rotation * Vec3::NEG_Z);
                    lo = lo.min(v);
                    hi = hi.max(v);
                    sum += v;
                }
            }
            let speed = sum / FIXED_TICKS_PER_SECOND as f32;
            let params = app
                .world()
                .resource::<crate::track::sim::TrackGear>()
                .trans();
            println!(
                "MEASURED shipped notch grade={degrees} pedal={pedal} capable={capable} supply={supply:?} speed={speed} spread={} gear={}",
                hi - lo,
                app.world().get::<TankTransmission>(tank).unwrap().0.gear
            );
            if capable || degrees == 0.0 {
                assert!(
                    speed > crate::track::transmission::PARK_ENGAGE_SPEED && speed > last,
                    "each capable notch must launch and increasing pedal must sustain increasing speed"
                );
                last = speed;
            } else {
                assert!(
                    speed.abs() < crate::track::transmission::PARK_ENGAGE_SPEED,
                    "an incapable notch must remain parked"
                );
            }
            assert!(
                hi - lo < 0.01 * params.governed_speed(false),
                "each notch must settle"
            );
        }
    }
}
