//! The game's driving and damage model, game state only: the Driver's resistance ledger and the
//! drivetrain response to a rearward shot. Crew are HP pools; this probe does not inspect HP
//! outcomes.

use super::*;
use crate::bitprobe::TransmissionProbe;
use crate::track::sim::{TankTransmission, TrackContacts, TrackDrive, TrackGripEffect};
use crate::track::transmission::{DriveSupply, price_driver_force};

/// Independent force-balance inverse for a freely motoring, unclamped drivetrain.
/// The positive root includes the ground law's linear rolling fade; a root beyond the
/// crank/clutch guards or inside the discrete crank-stop cap is outside this class.
pub(crate) fn coast_equilibrium(
    tp: &crate::track::transmission::TransmissionParams,
    gear: u8,
    weight: f32,
    angle: f32,
    rolling: f32,
) -> Option<f32> {
    use crate::track::transmission::{RPM_TO_RAD, overrev_margin_rpm};
    let k = tp.gears_fwd[usize::from(gear - 1)] / tp.sprocket_radius;
    let base = crate::track::transmission::motoring_drag_base_share();
    let anchor = tp.peak_torque_nm * tp.drag_fraction;
    let mid = (tp.engine.idle_rpm + tp.engine.governed_rpm) * 0.5 * RPM_TO_RAD;
    let grade = weight * angle.sin().abs();
    let rolling_load = weight * rolling * angle.cos();
    let coulomb = k * anchor * base;
    let slope = anchor * (1.0 - base) * k * k / mid;
    let fade = crate::track::forces::ROLLING_FADE_SPEED;
    let full_root = (grade - coulomb - rolling_load) / slope;
    let speed = if full_root >= fade {
        full_root
    } else {
        (grade - coulomb) / (slope + rolling_load / fade)
    };
    let omega = speed * k;
    let torque = anchor * (base + (1.0 - base) * omega / mid);
    (speed.is_finite()
        && speed > 0.0
        && omega < (tp.engine.crank_limit_rpm + overrev_margin_rpm()) * RPM_TO_RAD
        && torque < tp.clutch_capacity
        && torque <= tp.engine_inertia * omega * FIXED_TICKS_PER_SECOND as f32)
        .then_some(speed)
}

struct Landing {
    gear: u8,
    price: f32,
    speed: f32,
    first_delivery: Option<f32>,
    delivery: Vec<f32>,
    terms: Vec<[f32; 4]>,
}

/// `n`, minimum, median, 95th percentile and maximum of `values`, or `none`.
fn summary(values: &[f32]) -> String {
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
}

/// The sensed resistance this tick (N), when the Driver has a reading.
fn sensed_resistance(app: &App, tank: Entity) -> Option<f32> {
    let p = probe(app, tank);
    p.sensed.then_some(p.sensed_resistance_n)
}

#[derive(Default)]
struct Measurements {
    /// Sensed resistance over every tick, and split by cut.
    resist: Vec<f32>,
    resist_coupled: Vec<f32>,
    resist_cut: Vec<f32>,
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
        if probe.sensed {
            self.resist.push(probe.sensed_resistance_n);
            if probe.shifting {
                self.resist_cut.push(probe.sensed_resistance_n);
            } else {
                self.resist_coupled.push(probe.sensed_resistance_n);
            }
        }
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
            let speed = probe.sensed_speed
                - probe.sensed_resistance_n * f32::from(gear.trans().shift_ticks) * probe.dt
                    / gear.landing_mass();
            let price = price_driver_force(
                gear.trans(),
                DriveSupply::WHOLE,
                speed,
                probe.gear_reduction,
                1.0,
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
                let crank_price = probe.tau_free.min(tp.clutch_capacity) * probe.k;
                let acceleration = (probe.omega_integrated - probe.omega_pre) / probe.dt;
                let inertial_price = (probe.tau_free - tp.engine_inertia * acceleration)
                    .min(tp.clutch_capacity)
                    * probe.k;
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
        let margin = crate::track::transmission::DriverResolution::load().margin_n(tp);
        assert!(
            self.residuals
                .iter()
                .all(|gap| gap.is_finite() && gap.abs() < margin),
            "the measured deadband must cover the same-instant numerical residual"
        );
        println!(
            "MEASURED landing {label}: weight_n={weight:.6} sustained_gap_n[{}] landing_speed_error_m_s[{}]",
            summary(&self.gaps),
            summary(&self.landing_speed_errors)
        );
        println!(
            "MEASURED sensed {label}: expected_n={expected:.6} R[{}] R_coupled[{}] R_cut[{}]",
            summary(&self.resist),
            summary(&self.resist_coupled),
            summary(&self.resist_cut)
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
    let sense = crate::track::sense::DriverSense::for_cut(params.shift_ticks);
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
    *e.get_mut::<crate::track::sense::DriverSense>().unwrap() = sense;
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

/// The Driver's ledger must feel nominal weight times each declared ground's rolling coefficient
/// on the flat, independent of the Driver's entry path. The measurement records every coupled
/// landing; no particular ground, vehicle or gear is a pass condition. Fixture: uniform flat
/// ground, rest/top-gear entry, full command for 40 simulated seconds, last-second observation.
#[test]
fn sensed_resistance_and_landing_gaps_on_declared_grounds() {
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
                    measurements.resist.clear();
                }
            }
            if !rolling {
                println!(
                    "MEASURED launch {name}: time_to_0_5_m_s={:?} time_to_2_m_s={:?}",
                    initial_launch_tick.map(|ticks| ticks as f32 / FIXED_TICKS_PER_SECOND as f32),
                    launch_tick.map(|ticks| ticks as f32 / FIXED_TICKS_PER_SECOND as f32)
                );
            }
            let expected = ground.contact().rolling_at(nominal_pressure(&app)) * weight;
            measurements.report(
                &format!("{name} rolling={rolling}"),
                expected,
                weight,
                app.world()
                    .resource::<crate::track::sim::TrackGear>()
                    .trans(),
            );
            // Resolution: one window's ride ripple on the belt push, MEASURED within 8 N of the
            // coefficient at 559 kN on every shipped ground.
            for resistance in &measurements.resist {
                assert!(
                    (resistance - expected).abs() < weight * 1e-4,
                    "a uniform plane's resistance must match its own coefficient: {resistance} vs {expected}"
                );
            }
        }
    }
}

/// The ledger closes on an incline: a rolling hull feels `W sin θ + c_rr W cos θ` whether it
/// gains or loses speed, the gear law's one reading of the world. Parked, the contact law's
/// rolling fade leaves the grade alone. Fixture: uninterrupted 10/20/30 degree ramp planes,
/// parked then a rolling entry. Extending the faces isolates their support from the flat slab
/// and short course crests.
#[test]
fn sensed_resistance_and_landing_gaps_on_the_course() {
    for degrees in [10.0_f32, 20.0, 30.0] {
        let angle = degrees.to_radians();
        let grid = driving_plane(angle);
        let mut app = booted_driving_grid(grid);
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
            .rolling_at(nominal_pressure(&app));
        let expected = weight * (angle.sin() + rolling * angle.cos());
        let mut measurements = Measurements::default();
        let mut parked = Vec::new();
        for _ in 0..64 {
            drive_tick(&mut app, tank, 0.0, 0.0);
            measurements.tick(&app, tank);
            parked.extend(sensed_resistance(&app, tank));
        }
        println!(
            "MEASURED sensed parked {degrees} degrees: grade_n={:.6} R[{}]",
            weight * angle.sin(),
            summary(&parked)
        );
        // A settled reading does not imply a resting body. The launch proof starts with fresh
        // motion, strain and drivetrain state on that same supported pose.
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
        let mut slip_ticks = 0;
        let mut cuts = 0;
        let mut was_cut = false;
        let mut minimum = 0.0_f32;
        let (mut launch_coupled, mut launch_cut) = (Vec::new(), Vec::new());
        for tick in 0..30 * FIXED_TICKS_PER_SECOND {
            drive_tick(&mut app, tank, 1.0, 0.0);
            let p = probe(&app, tank);
            // The launch gear is the one the clutch first engages; a fresh placement has felt
            // nothing on its first tick.
            if launch_gear == 0 && p.engaged {
                launch_gear = app.world().get::<TankTransmission>(tank).unwrap().0.gear;
            }
            // The first second holds the launch's own slip; the rest is the run-up.
            if p.sensed && tick >= FIXED_TICKS_PER_SECOND {
                if p.shifting {
                    launch_cut.push(p.sensed_resistance_n);
                } else {
                    launch_coupled.push(p.sensed_resistance_n);
                }
            }
            cuts += usize::from(p.shifting && !was_cut);
            slip_ticks += usize::from(
                app.world().get::<TankTransmission>(tank).unwrap().0.clutch
                    == crate::track::transmission::ClutchState::Slip,
            );
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
            "MEASURED grade launch degrees={degrees} gear={launch_gear} distance_m={climb} cuts={cuts} slip_s={} minimum_belt_m_s={minimum}",
            slip_ticks as f32 / FIXED_TICKS_PER_SECOND as f32
        );
        println!(
            "MEASURED sensed launch {degrees} degrees: expected_n={expected:.6} R_coupled[{}] R_cut[{}]",
            summary(&launch_coupled),
            summary(&launch_cut)
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
        let tp = app
            .world()
            .resource::<crate::track::sim::TrackGear>()
            .trans();
        // Fixture: mid-band crank in this construction's upper-middle gear.
        let gear = (tp.gears_fwd.len() * 3 / 4).max(1) as u8;
        let rpm = (tp.engine.idle_rpm + tp.engine.governed_rpm) * 0.5;
        let speed = rpm * crate::track::transmission::RPM_TO_RAD * tp.sprocket_radius
            / tp.gears_fwd[usize::from(gear - 1)];
        place(&mut app, tank, position, rotation, speed, gear);
        let mut rolling_entry = Vec::new();
        for _ in 0..512 {
            drive_tick(&mut app, tank, 1.0, 0.0);
            measurements.tick(&app, tank);
            let p = probe(&app, tank);
            if p.sensed && !p.shifting {
                rolling_entry.push(p.sensed_resistance_n);
            }
        }
        rolling_entry.sort_by(f32::total_cmp);
        let median = rolling_entry[rolling_entry.len() / 2];
        // Resolution: the ride's ripple on the median coupled reading, MEASURED within 10 N of
        // the derived load at 559 kN on each plane.
        assert!(
            (median - expected).abs() < weight * 1e-4,
            "a rolling hull must feel its incline whatever its acceleration: {median} vs {expected}"
        );
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

/// The game's driving and damage model: a rearward shot moves the hull, its contact forces and
/// the Driver's reading, but must not change a gear decision or start a shift. Fixture: part
/// command on the course's middle incline, a rear-facing primary weapon, paired deterministic
/// runs with and without one shot. HP outcomes are outside this probe.
#[test]
fn a_rearward_shot_uphill_changes_no_gear() {
    let run = |fire: bool| {
        let angle = 20.0_f32.to_radians();
        // Fixture: an uninterrupted incline; recoil cannot move a patch across a ramp edge.
        let grid = driving_plane(angle);
        let mut app = booted_driving_grid(grid);
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
        let mut resist = Vec::new();
        let mut gears = Vec::new();
        for tick in 0..128 {
            app.world_mut()
                .get_mut::<TankCommand>(tank)
                .unwrap()
                .fire_primary = fire && tick == 64;
            drive_tick(&mut app, tank, 0.4, 0.0);
            resist.push(sensed_resistance(&app, tank));
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
        (resist, gears)
    };
    let (baseline_resist, baseline_gears) = run(false);
    let (fired_resist, fired_gears) = run(true);
    let resist_change: Vec<f32> = baseline_resist
        .iter()
        .zip(&fired_resist)
        .skip(64)
        .filter_map(|(a, b)| Some(b.as_ref()? - a.as_ref()?))
        .collect();
    println!(
        "MEASURED sensed rearward shot uphill: baseline R after the shot[{}] paired R change (fired - baseline)[{}]",
        summary(
            &baseline_resist[64..]
                .iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>()
        ),
        summary(&resist_change)
    );
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
    use crate::track::transmission::{ClutchState, RPM_TO_RAD, overrev_margin_rpm};
    let mut observed_roots = 0;
    for degrees in [-10.0_f32, 10.0] {
        for launch_gear in [true, false] {
            let angle = degrees.to_radians();
            let grid = driving_plane(angle);
            let mut app = booted_driving_grid(grid);
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
                tp.launch_gears.min(tp.gears_fwd.len() as u8)
            } else {
                (tp.gears_fwd.len() * 3 / 4).max(1) as u8
            };
            let guard = (tp.engine.crank_limit_rpm + overrev_margin_rpm()) * RPM_TO_RAD;
            let weight =
                app.world().resource::<TankBlueprint>().spec.mass * crate::track::derive::G;
            let rolling = crate::ground_types::GroundTypes::load_shipped()
                .get(crate::terrain_blend::BASE_GROUND)
                .contact()
                .rolling_at(nominal_pressure(&app));
            let equilibrium = coast_equilibrium(tp, gear, weight, angle, rolling);
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
                assert!(
                    app.world()
                        .get::<TrackContacts>(tank)
                        .unwrap()
                        .0
                        .iter()
                        .any(|side| !side.is_empty())
                );
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
            // None is outside the inverse's unclamped class; drag and guard checks still apply.
            if let Some(expected) = equilibrium {
                observed_roots += 1;
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
    assert!(
        observed_roots > 0,
        "S3 must deliver at least one unclamped motoring root"
    );
}

/// The shipped composition must launch at every player notch on level ground and on a
/// supported grade, and sustain increasing steady travel. Fixture: a long firm plane.
#[test]
fn shipped_notches_launch_and_sustain_ordered_speed() {
    for degrees in [0.0_f32, 10.0] {
        let mut last = 0.0;
        let count = crate::track::drive::drive_feel().notch_count.0;
        for index in 1..=count {
            let pedal = f32::from(index) / f32::from(count);
            let angle = degrees.to_radians();
            let grid = driving_plane(angle);
            let mut app = booted_driving_grid(grid);
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
            // The launch starts parked: the settle builds the grip strain and the Driver's ledger,
            // which a placement resets and play never does.
            for _ in 0..128 {
                drive_tick(&mut app, tank, 0.0, 0.0);
            }
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
                    let resistance = sensed_resistance(&app, tank)
                        .expect("the settled park has filled the Driver's ledger");
                    capable = (1..=tp.launch_gears).any(|g| {
                        price_driver_force(
                            tp,
                            supply,
                            0.0,
                            tp.gears_fwd[usize::from(g - 1)],
                            pedal,
                            crate::track::transmission::ClutchState::Slip,
                        ) > resistance
                    });
                }
                assert!(
                    app.world()
                        .get::<TrackContacts>(tank)
                        .unwrap()
                        .0
                        .iter()
                        .any(|side| !side.is_empty()),
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
