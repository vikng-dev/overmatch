//! Inline driving fixtures shared by the transmission's and the Driver's arithmetic tests and by
//! spec/contact composition tests.

use super::drive::{DriveAxes, DriveIntent};
use super::driver::Sensed;
use super::forces;
use super::transmission::{
    ClutchState, Coupling, DriveConstruction, DriveSupply, Output, TransmissionAuthoring,
    TransmissionInput, TransmissionParams, TransmissionReport, TransmissionState,
    drag_throttle_release, motoring_drag_base_share, price_driver_force,
};

/// Fixture: a lighter three-speed construction using the seed's declared running-gear geometry.
/// Its engine has a tail past the governor, lower power/weight and a distinct gear ladder.
pub(crate) fn compact_spec() -> crate::spec::TankSpec {
    let mut spec = super::rig_geom::tiger_spec();
    spec.mass = 18_000.0;
    let tr = &mut spec.track.powertrain.transmission;
    tr.engine.idle_rpm = 700.0;
    tr.engine.governed_rpm = 2200.0;
    tr.engine.crank_limit_rpm = crate::provenance::Sourced(2600.0); // Fixture limit.
    tr.engine.rated_rpm = 2200.0;
    tr.engine.torque_curve = vec![
        (700.0, 600.0),
        (1400.0, 800.0),
        (2200.0, 500.0),
        (2600.0, 400.0),
    ];
    tr.engine.drag_fraction = 0.3;
    tr.engine.inertia_kgm2 = 2.0;
    tr.engine.clutch_capacity_nm = 1100.0;
    tr.gearbox.forward_speeds_kmh = vec![5.0, 12.0, 25.0];
    tr.gearbox.reverse_speeds_kmh = vec![10.0];
    tr.gearbox.launch_gears = crate::provenance::Sourced(2); // Fixture limit.
    tr.gearbox.max_step = crate::provenance::Sourced(1); // Fixture reach.
    tr.gearbox.shift_secs = crate::provenance::Sourced(0.2); // Fixture interruption.
    tr.steering.radii = vec![(3.0, 8.0), (7.2, 19.2), (15.0, 40.0)];
    tr.steering.reverse_radii = vec![(6.0, 16.0)];
    tr.steering.capacity = Some(80_000.0);
    tr.rated_slope_deg = crate::provenance::Labelled {
        value: 30.0,
        label: crate::provenance::Provenance::Fixture("compact rated climbing slope".into()),
    };
    tr.mean_hold = Some(crate::provenance::Provenance::StandIn(
        "compact gearbox-brake torque and reduction missing; rated-grade rule supplies mean capacity".into(),
    ));
    tr.brake_static_factor = crate::provenance::Labelled {
        value: 1.4,
        label: crate::provenance::Provenance::Fixture("compact static breakaway multiplier".into()),
    };
    spec
}

/// Fixture: a firmer loaded surface with a friction coefficient distinct from the lab's.
pub(crate) fn ground() -> crate::ground_field::ContactGround {
    crate::ground_field::ContactGround {
        grip: 0.65,
        rolling_resistance: 0.04,
        shear_length_m: 0.010,
    }
}

/// Fixture: what a drivetrain's contacts grip with and its belts' reflected inertia (kg).
#[derive(Clone)]
pub(crate) struct FixtureConstruction {
    pub(crate) grip: forces::GripParams,
    pub(crate) ground_grip: f32,
    pub(crate) inertia: f32,
}

/// Fixture: contact grip and reflected belt inertia for the arithmetic drivetrain.
pub(crate) fn lab_fp() -> FixtureConstruction {
    FixtureConstruction {
        inertia: 8_000.0,
        ground_grip: 0.9, // Fixture terrain coefficient, independent of track material.
        grip: forces::GripParams {
            track: crate::substances::contact_friction(0.9),
            nominal_pressure_kpa: forces::rolling_reference_kpa(),
            side_weight: 26_500.0 * 9.81 / 2.0,
            hull_coupling: 1.0, // Fixture point-mass hull.
        },
    }
}

/// Fixture adapter: arithmetic contact fixtures always carry a driven material pair.
pub(crate) fn construction(params: &FixtureConstruction) -> DriveConstruction {
    DriveConstruction::new(params.grip, params.inertia)
}

pub(crate) fn step(
    fp: &FixtureConstruction,
    tp: &TransmissionParams,
    state: &mut TransmissionState,
    input: &TransmissionInput,
) -> TransmissionReport {
    let fp = construction(fp);
    let orders = super::transmission::prepare_orders(&fp, tp, state, input);
    let mut mechanical = super::transmission::DriveState {
        transmission: *state,
        speeds: input.speeds,
        phases: [0.0; 2],
    };
    let report = super::transmission::advance(&fp, tp, &mut mechanical, &orders, input.dt);
    *state = mechanical.transmission;
    report
}

/// One Driver decision and the physics clock's substep call sequence, for tick-level invariants.
pub(crate) fn substep_tick(
    fp: &FixtureConstruction,
    tp: &TransmissionParams,
    state: &mut TransmissionState,
    input: &TransmissionInput,
) -> TransmissionReport {
    let fp = construction(fp);
    let mut tick = super::transmission::DriveTick::new(
        &fp,
        tp,
        super::transmission::DriveState {
            transmission: *state,
            speeds: input.speeds,
            phases: [0.0; 2],
        },
        input,
    );
    let count = avian3d::prelude::SubstepCount::default().0;
    let h = std::time::Duration::from_secs_f32(input.dt)
        .div_f64(f64::from(count))
        .as_secs_f32();
    for _ in 0..count {
        tick.advance(&fp, tp, h);
    }
    *state = tick.state.transmission;
    tick.report(count)
}

/// Fixture: a second torque curve and ladder, independent of the shipped vehicle.
pub(crate) fn lab_tp() -> TransmissionParams {
    TransmissionParams::from_authoring(&lab_authoring())
        .expect("lab transmission authoring must be valid")
}

pub(crate) fn lab_authoring() -> TransmissionAuthoring<'static> {
    TransmissionAuthoring {
        idle_rpm: 600.0,
        governed_rpm: 1800.0,
        crank_limit_rpm: 1900.0, // Fixture: completed fuel cut.
        rated_rpm: 1800.0,
        torque_nm: &[
            (600.0, 1650.0),
            (1100.0, 2200.0),
            (1700.0, 1950.0),
            (1800.0, 0.0),
        ],
        forward_speeds_kmh: &[8.0, 12.7, 20.4, 32.6, 52.2],
        reverse_speeds_kmh: &[8.0],

        launch_gears: 4,
        max_step: 3,
        steer_radii_m: &[
            (3.0, 8.9),
            (4.8, 14.2),
            (7.7, 22.8),
            (12.3, 36.4),
            (19.7, 58.3),
        ],
        reverse_steer_radii_m: &[(3.0, 8.9)],
        steer_capacity_n: 240_000.0,
        couplings: &[
            Coupling::Clutch(Output::Mean),
            Coupling::Steering,
            Coupling::Stop(Output::Left),
            Coupling::Stop(Output::Right),
        ],
        steering_radius_rounding_share: 0.01, // Fixture: table rounding allowance.
        brake_capacity_n: 120_000.0,
        mean_hold_capacity_n: None,
        brake_static_factor: 1.6,
        drag_fraction: 0.25,
        // Fixture: six kg m² resolves this curve's idle catch (DERIVED minimum about five).
        // Clutch capacity is about 1.3 times the fixture peak torque.
        engine_inertia_kgm2: 6.0,
        clutch_capacity_nm: 2860.0,
        belt_inertia: 8_000.0,
        shift_secs: 0.31,

        sprocket_radius_m: 0.34,
        half_tread_m: 1.25,
    }
}

/// FIXTURE DATA: per-side belt inertia (kg) at the scale the shipped sheet's construction sum
/// derives (DERIVED 4,749.86 kg, `reflected-track-inertia.md`), rounded. The arithmetic tests
/// assert gear choices at this coupling; none is a pass condition on the shipped vehicle.
pub(crate) const TIGER_BELT_INERTIA: f32 = 4_750.0;

/// FIXTURE DATA modelled on the shipped Tiger's declared drivetrain: the torque curve, speed
/// anchors, steering table, clutch and brakes as tiger_1.tank.ron states them. The sprocket radius, half-tread and belt inertia are fixture
/// values at the sheet's scale, not its measured or derived ones. Kept local to arithmetic tests
/// so they exercise the authored curve and speed anchors without reaching through the ECS/spec
/// adapter. Exposed as the authoring struct so validator tests can mutate one field and
/// re-validate.
pub(crate) fn tiger_authoring() -> TransmissionAuthoring<'static> {
    let spec = super::rig_geom::tiger_spec();
    let brakes = &spec.track.powertrain.transmission;
    TransmissionAuthoring {
        idle_rpm: 600.0,
        governed_rpm: 2500.0,
        crank_limit_rpm: 3000.0,
        rated_rpm: 3000.0,
        torque_nm: &[
            (800.0, 1300.0),
            (2100.0, 1850.0),
            (2500.0, 1686.0),
            (3000.0, 1639.0),
        ],
        forward_speeds_kmh: &[2.8, 4.3, 6.2, 9.2, 14.1, 20.9, 30.5, 45.4],
        reverse_speeds_kmh: &[2.8, 4.3, 6.2, 9.2],

        launch_gears: 4,
        max_step: 3,
        steer_radii_m: &[
            (3.44, 10.2),
            (5.28, 15.6),
            (7.62, 22.5),
            (11.30, 33.4),
            (17.32, 51.2),
            (25.68, 76.0),
            (37.47, 110.8),
            (55.78, 165.0),
        ],
        reverse_steer_radii_m: &[(3.44, 10.2), (5.28, 15.6), (7.62, 22.5), (11.30, 33.4)],
        steer_capacity_n: 250_000.0,
        couplings: &[
            Coupling::Clutch(Output::Mean),
            Coupling::Steering,
            Coupling::Stop(Output::Left),
            Coupling::Stop(Output::Right),
            Coupling::Stop(Output::Mean),
        ],
        steering_radius_rounding_share: 0.01, // Fixture: table rounding allowance.
        brake_capacity_n: brakes
            .brake_capacity_n(spec.mass)
            .expect("the fixture's rated grade is admitted"),
        mean_hold_capacity_n: brakes
            .mean_hold_capacity_n(spec.mass)
            .expect("the fixture's mean brake is admitted"),
        brake_static_factor: brakes.brake_static_factor.value,
        drag_fraction: 0.25,
        engine_inertia_kgm2: 4.0,
        clutch_capacity_nm: 2400.0,
        belt_inertia: TIGER_BELT_INERTIA,
        shift_secs: 0.31,

        sprocket_radius_m: 19.0 * 0.130 / std::f32::consts::TAU,
        half_tread_m: 1.4904,
    }
}

pub(crate) fn tiger_tp() -> TransmissionParams {
    TransmissionParams::from_authoring(&tiger_authoring())
        .expect("Tiger transmission authoring must be valid")
}

/// Fixture: a lighter three-speed drivetrain with its own curve, ratios and capacities.
pub(crate) fn compact_drivetrain() -> (FixtureConstruction, TransmissionParams) {
    let spec = compact_spec();
    let ground = ground();
    let mut fp = lab_fp();
    fp.inertia = 3000.0; // Fixture belt inertia, independent of the contact composition's geometry.
    fp.ground_grip = ground.grip;
    fp.grip = forces::GripParams {
        track: spec.track.friction(),

        nominal_pressure_kpa: forces::rolling_reference_kpa(),
        side_weight: spec.mass * super::derive::G / 2.0,
        hull_coupling: 1.0, // Fixture point-mass hull.
    };
    let tp = spec
        .track
        .powertrain
        .transmission
        .params(0.3, 1.0, fp.inertia, spec.mass)
        .expect("inline Fixture drivetrain must be admitted");
    (fp, tp)
}

/// Fixture: side clutches and brakes, with gear-independent radii and no geared steering member.
pub(crate) fn side_clutch_drivetrain() -> (FixtureConstruction, TransmissionParams) {
    let mut a = lab_authoring();
    a.couplings = &[
        Coupling::Clutch(Output::Left),
        Coupling::Clutch(Output::Right),
        Coupling::Stop(Output::Left),
        Coupling::Stop(Output::Right),
    ];
    a.steer_radii_m = &[(3.0, 9.0); 5];
    a.reverse_steer_radii_m = &[(3.0, 9.0)];
    a.steer_capacity_n = 0.0;
    let mut spec = super::rig_geom::tiger_spec();
    let brakes = &mut spec.track.powertrain.transmission;
    brakes.rated_slope_deg.value = 28.0; // Fixture: this construction's own climb rating.
    a.brake_capacity_n = brakes
        .brake_capacity_n(2.0 * lab_fp().grip.side_weight / super::derive::G)
        .expect("the fixture's rated grade is admitted");
    (
        lab_fp(),
        TransmissionParams::from_authoring(&a)
            .expect("Fixture side-clutch construction is admitted"),
    )
}

pub(crate) fn class_drivetrains() -> [(FixtureConstruction, TransmissionParams); 4] {
    [
        (lab_fp(), lab_tp()),
        (grade_fixture_fp(), tiger_tp()),
        compact_drivetrain(),
        side_clutch_drivetrain(),
    ]
}

pub(crate) fn input(
    throttle: f32,
    steer: f32,
    speeds: [f32; 2],
    reactions: [f32; 2],
) -> TransmissionInput {
    TransmissionInput {
        supply: DriveSupply::WHOLE,
        intent: DriveIntent::Axes(DriveAxes { throttle, steer }),
        throttle,
        steer,
        side_commands: [
            (throttle + steer).clamp(-1.0, 1.0),
            (throttle - steer).clamp(-1.0, 1.0),
        ],
        speeds,
        reactions,
        // Fixture: reactions that do not move with belt speed.
        reaction_slopes: [0.0; 2],
        // Fixture: the Driver has felt nothing, so it neither chooses a gear nor engages one from
        // rest; a launch reads `felt_at_rest`, Driver policy `driver_input`.
        sensed: None,
        dt: 1.0 / 64.0,
    }
}

pub(crate) fn fresh(tp: &TransmissionParams) -> TransmissionState {
    TransmissionState {
        park: false,
        ..TransmissionState::from_spec(tp)
    }
}

/// Fixture: nominal 57-tonne construction and its derived 4,750 kg reflected belt.
pub(crate) fn grade_fixture_fp() -> FixtureConstruction {
    let mut fp = lab_fp();
    fp.inertia = TIGER_BELT_INERTIA;
    fp.grip.side_weight = super::rig_geom::tiger_spec().mass * super::derive::G / 2.0;
    fp
}

/// Fixture Driver reading for Driver-policy tests: a hull riding its belts at `speed` and
/// meeting `load` along its heading (forward positive), independent of dynamic belt reactions,
/// so acceleration cannot manufacture a gear change, on ground that grips beyond any gear's pull.
pub(crate) fn driver_input(speed: f32, load: f32, command: f32) -> TransmissionInput {
    let mut inp = input(command, 0.0, [speed; 2], [0.0; 2]);
    inp.sensed = Some(Sensed::steady(speed, load, f32::INFINITY));
    inp
}

/// Fixture: a pedal whose own fuel nets nothing at any crank speed on [`DriveSupply::WHOLE`],
/// derived from the engine's parameters, so an engine retune moves it and keeps it braking. At
/// command `p` the crank nets `p·T + s·c·D − D` (`transmission::engine_torques`): the curve `T` at
/// most its peak `P`, the governor's cut `c` at most one, the release `s` the smoothstep of `p`
/// over `drag_throttle_release` `r`, and the motoring drag `D` at least its Coulomb base `b·f·P`
/// (`motoring_drag_base_share` `b`, `drag_fraction` `f`). Half the lesser of `b·f` and `r` keeps
/// `s` at most `s(½) = ½`, so the net is at most `P·(p − b·f/2) ≤ 0` (DERIVED).
pub(crate) fn braking_pedal(tp: &TransmissionParams) -> f32 {
    0.5 * (motoring_drag_base_share() * tp.drag_fraction).min(drag_throttle_release())
}

/// The fixture reading a Driver-policy input carries.
pub(crate) fn felt(inp: &TransmissionInput) -> Sensed {
    inp.sensed
        .expect("a Driver-policy fixture carries a reading")
}

/// Fixture oracle: enumerate the declared straight-launch domain using the separately pinned
/// crank/cap force price, viable at or above the felt resistance. This never consults the selector.
pub(crate) fn affordable_launch(
    tp: &TransmissionParams,
    inp: &TransmissionInput,
    command: f32,
) -> Option<u8> {
    (1..=tp.launch_gears.min(tp.gears_fwd.len() as u8))
        .rev()
        .find(|&gear| {
            price_driver_force(
                tp,
                inp.supply,
                0.0,
                tp.gears_fwd[usize::from(gear - 1)],
                command,
                ClutchState::Slip,
            ) >= felt(inp).resistance
        })
}
