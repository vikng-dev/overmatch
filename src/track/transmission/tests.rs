use super::super::drive::DriveAxes;
use super::super::forces;
use super::super::sense::Sensed;
use super::*;

#[derive(Clone)]
struct FixtureConstruction {
    grip: forces::GripParams,
    ground_grip: f32,
    inertia: f32,
}

/// Fixture: contact grip and reflected belt inertia for the arithmetic drivetrain.
fn lab_fp() -> FixtureConstruction {
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
fn construction(params: &FixtureConstruction) -> DriveConstruction {
    DriveConstruction::new(params.grip, params.inertia)
}

fn step(
    fp: &FixtureConstruction,
    tp: &TransmissionParams,
    state: &mut TransmissionState,
    input: &TransmissionInput,
) -> TransmissionReport {
    super::step(&construction(fp), tp, state, input)
}

/// Fixture: a second torque curve and ladder, independent of the shipped vehicle.
fn lab_tp() -> TransmissionParams {
    TransmissionParams::from_authoring(&TransmissionAuthoring {
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
        neutral_mean: NeutralMean::Free,
        recirculation: 0.9,
        brake_capacity_n: 120_000.0,
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
    })
    .expect("lab transmission authoring must be valid")
}

/// FIXTURE DATA: per-side belt inertia (kg) at the scale the shipped sheet's construction sum
/// derives (DERIVED 4,749.86 kg, `reflected-track-inertia.md`), rounded. The arithmetic tests
/// assert gear choices at this coupling; none is a pass condition on the shipped vehicle.
const TIGER_BELT_INERTIA: f32 = 4_750.0;

/// FIXTURE DATA modelled on the shipped Tiger's declared drivetrain: the torque curve, speed
/// anchors, steering table, clutch and brakes as tiger_1.tank.ron states them. The sprocket radius, half-tread and belt inertia are fixture
/// values at the sheet's scale, not its measured or derived ones. Kept local to arithmetic tests
/// so they exercise the authored curve and speed anchors without reaching through the ECS/spec
/// adapter. Exposed as the authoring struct so validator tests can mutate one field and
/// re-validate.
fn tiger_authoring() -> TransmissionAuthoring<'static> {
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
        neutral_mean: NeutralMean::Held,
        recirculation: 0.9,
        brake_capacity_n: 96_000.0,
        brake_static_factor: 1.5,
        drag_fraction: 0.25,
        engine_inertia_kgm2: 4.0,
        clutch_capacity_nm: 2400.0,
        belt_inertia: TIGER_BELT_INERTIA,
        shift_secs: 0.31,

        sprocket_radius_m: 19.0 * 0.130 / std::f32::consts::TAU,
        half_tread_m: 1.4904,
    }
}

fn tiger_tp() -> TransmissionParams {
    TransmissionParams::from_authoring(&tiger_authoring())
        .expect("Tiger transmission authoring must be valid")
}

/// Fixture: a lighter three-speed drivetrain with its own curve, ratios and capacities.
fn compact_drivetrain() -> (FixtureConstruction, TransmissionParams) {
    let spec = super::super::drive_fixture::compact_spec();
    let ground = super::super::drive_fixture::ground();
    let mut fp = lab_fp();
    fp.inertia = 3000.0; // Fixture belt inertia, independent of the contact composition's geometry.
    fp.ground_grip = ground.grip;
    fp.grip = forces::GripParams {
        track: spec.track.friction(),

        nominal_pressure_kpa: forces::rolling_reference_kpa(),
        side_weight: spec.mass * super::super::derive::G / 2.0,
        hull_coupling: 1.0, // Fixture point-mass hull.
    };
    let tp = spec
        .track
        .powertrain
        .transmission
        .params(0.3, 1.0, fp.inertia)
        .expect("inline Fixture drivetrain must be admitted");
    (fp, tp)
}

fn class_drivetrains() -> [(FixtureConstruction, TransmissionParams); 3] {
    [
        (lab_fp(), lab_tp()),
        (grade_fixture_fp(), tiger_tp()),
        compact_drivetrain(),
    ]
}

fn input(throttle: f32, steer: f32, speeds: [f32; 2], reactions: [f32; 2]) -> TransmissionInput {
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

/// Fixture: the Driver has felt a hull at rest meeting nothing, on ground that grips beyond any
/// gear's pull. From an open clutch it launches in its highest launch gear; once engaged, no gear
/// launches with more than the one it holds, so no gear changes.
fn felt_at_rest(mut inp: TransmissionInput) -> TransmissionInput {
    inp.sensed = Some(Sensed {
        speed: 0.0,
        resistance: 0.0,
        push_difference: 0.0,
        drive: 0.0,
        drive_gain: 0.0,
        grip: Some(f32::INFINITY),
        stall: None,
        failed: None,
        stalled: false,
        observed: true,
    });
    inp
}

/// Fixture: `felt_at_rest` with belts that meet no ground, which gives them no grip to carry a
/// pull, so no gear offers more than the gear engaged.
fn felt_free_at_rest(inp: TransmissionInput) -> TransmissionInput {
    let mut inp = felt_at_rest(inp);
    inp.sensed.as_mut().expect("the fixture feels").grip = Some(0.0);
    inp
}

fn fresh(tp: &TransmissionParams) -> TransmissionState {
    TransmissionState {
        park: false,
        ..TransmissionState::from_spec(tp)
    }
}

/// The HUD readout reports the CRANK STATE ω_e directly — the state IS the
/// display. Freshly constructed, it reads idle; driving in reverse it reads the
/// crank's geared speed with the R label; back-driven forward while in R, the crank cannot
/// follow the negative shaft — the stall guard keeps ω_e
/// idle-ish, and the readout shows exactly that state, never a fake forward rpm.
#[test]
fn readout_reports_crank_state() {
    let (fp, tp) = (lab_fp(), lab_tp());
    // Spawn-constructed, never stepped: idle.
    let st = TransmissionState {
        reverse: true,
        ..fresh(&tp)
    };
    let r = readout(&st, &tp);
    assert_eq!(r.gear_label, "R1");
    assert_eq!(
        r.rpm, tp.engine.idle_rpm,
        "fresh spec state must read authored idle"
    );
    // Fixture: reverse crank rotation, independent of the selected ladder.
    let backward = TransmissionState {
        omega_e: -tp.engine.idle_rpm * RPM_TO_RAD,
        ..st
    };
    assert_eq!(readout(&backward, &tp).rpm, -tp.engine.idle_rpm);
    // Driving in reverse at a steady R1 speed: the lock puts the crank AT the geared
    // speed of the belt the transmission itself integrated (`k·s·m_next` — with this
    // harness holding the INPUT speeds externally, the belt it computes each tick sits
    // `k·τ_free·dt/I_m` above the held value, and the crank rides THAT belt exactly).
    let mut st = TransmissionState {
        reverse: true,
        ..fresh(&tp)
    };
    let mut rep = TransmissionReport::default();
    for _ in 0..64 {
        rep = step(
            &fp,
            &tp,
            &mut st,
            &input(-1.0, 0.0, [-2.0, -2.0], [0.0, 0.0]),
        );
    }
    let r = readout(&st, &tp);
    assert_eq!(r.gear_label, "R1");
    let m_next = (rep.next_speeds[0] + rep.next_speeds[1]) / 2.0;
    let geared = -m_next * tp.gears_rev[0] / tp.sprocket_radius / RPM_TO_RAD;
    assert!(
        geared > tp.engine.idle_rpm && (r.rpm - geared).abs() < 25.0,
        "driving in reverse, the crank readout must sit at the geared rpm of the \
             integrated belt ({geared:.0}), got {:.0}",
        r.rpm
    );
    // Back-driven while in R (rolling forward, shaft < 0): the crank never follows —
    // the stall guard bounds it at idle − engine_control().idle_guard_band_rpm.value, and the readout shows
    // the honest idle-ish crank, not a fake geared rpm.
    let mut st = TransmissionState {
        reverse: true,
        ..fresh(&tp)
    };
    for _ in 0..64 {
        step(
            &fp,
            &tp,
            &mut st,
            &input(-1.0, 0.0, [2.0, 2.0], [-40_000.0, -40_000.0]),
        );
    }
    let r = readout(&st, &tp);
    assert_eq!(r.gear_label, "R1");
    assert!(
        r.rpm >= tp.engine.idle_rpm - engine_control().idle_guard_band_rpm.value - 1.0
            && r.rpm <= tp.engine.governed_rpm,
        "a back-driven R shaft must read the idle-ish crank (≥ idle − band), got {:.0}",
        r.rpm
    );
}

/// The brake hold: a rolling tank brakes to a stop and stays parked, never through zero into
/// reverse, on a FIXTURE downhill pull well inside both brake capacities. The repair and the
/// Battle's command hold rely on it (`track::sim::apply_track_forces`); the same input with the brakes
/// released keeps rolling, so a hold that only zeroes the command fails.
#[test]
fn brake_hold_stops_a_rolling_tank_and_parks_it() {
    let (fp, tp) = (lab_fp(), lab_tp());
    // FIXTURE: 6 m/s in gear 4, a 5 kN-per-side downhill pull.
    let run = |brake: bool| {
        let mut st = TransmissionState {
            gear: 4,
            ..fresh(&tp)
        };
        let mut speeds = [6.0f32, 6.0];
        let mut trace = Vec::new();
        for _ in 0..320 {
            let inp = TransmissionInput {
                supply: DriveSupply::WHOLE,
                intent: if brake {
                    DriveIntent::Brake
                } else {
                    DriveIntent::Axes(DriveAxes::default())
                },
                ..input(0.0, 0.0, speeds, [-5_000.0, -5_000.0])
            };
            speeds = step(&fp, &tp, &mut st, &inp).next_speeds;
            trace.push((speeds, st.park));
        }
        trace
    };
    let held = run(true);
    let stopped = held
        .iter()
        .position(|(speeds, park)| *park && speeds.iter().all(|v| v.abs() < PARK_ENGAGE_SPEED))
        .expect("the brake hold must bring the tank to a parked rest");
    // DERIVED bound: (2 × 120 kN − 10 kN) over 16 t arrests 6 m/s in about 27 ticks.
    assert!(stopped < 64, "the brake hold took {stopped} ticks to park");
    for (tick, (speeds, park)) in held.iter().enumerate().skip(stopped) {
        assert!(
            *park && speeds.iter().all(|v| v.abs() < PARK_ENGAGE_SPEED),
            "tick {tick}: a held tank stays parked, got {speeds:?}"
        );
    }
    assert!(
        held.iter()
            .all(|(speeds, _)| speeds.iter().all(|v| *v > -PARK_ENGAGE_SPEED)),
        "the brake hold never drives the belt through zero"
    );
    let coasting = run(false);
    let (speeds, _) = coasting[stopped];
    assert!(
        speeds.iter().all(|v| *v > 1.0),
        "a released command coasts: the hold must be the brakes, got {speeds:?}"
    );
}

/// Releasing steer makes the neutral-turn servo target zero belt difference. With no
/// ground reactions the servo alone must arrest the pivot, so the driver's released stick
/// cannot leave the vehicle turning indefinitely.
#[test]
fn steer_release_arrests_pivot() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    let mut speeds = [0.0f32; 2];
    // Spin up a standstill pivot (zero reactions — the worst case: nothing external
    // ever damps the belts).
    for _ in 0..64 {
        let rep = step(&fp, &tp, &mut st, &input(0.0, 1.0, speeds, [0.0, 0.0]));
        speeds = rep.next_speeds;
    }
    let d0 = (speeds[0] - speeds[1]) / 2.0;
    assert!(d0 > 0.1, "the pivot must actually be turning (d = {d0})");
    // Fixture observation budget: one simulated minute for monotonic arrest without reversal.
    let mut previous = d0;
    for _ in 0..64 * 60 {
        let rep = step(&fp, &tp, &mut st, &input(0.0, 0.0, speeds, [0.0, 0.0]));
        speeds = rep.next_speeds;
        let difference = (speeds[0] - speeds[1]) / 2.0;
        assert!(
            difference >= 0.0 && difference <= previous,
            "released pivot must decay without reversal"
        );
        if difference == 0.0 {
            return;
        }
        previous = difference;
    }
    panic!("released pivot must reach rest");
}

/// A neutral pivot on a slope: with the mean path held (`NeutralMean::Held`) the belts run equal
/// and opposite, the mean at rest while the difference reaches `neutral_d_full`; with it free the
/// grade sets the mean. A pivot that turns in place on a slope relies on the held fact, and a
/// held mean must not cost the difference its target: the servo is kinematic inside its
/// capacity, so the difference lands exactly even where the two belts' grounds answer unequally
/// and the hold's even split would move it.
/// Fixture: unequal reaction slopes, a fraction of each belt's inertia per tick.
#[test]
fn a_held_mean_pivots_in_place_on_a_slope_and_a_free_mean_does_not() {
    let fp = lab_fp();
    for mean in [NeutralMean::Held, NeutralMean::Free] {
        let tp = TransmissionParams {
            neutral_mean: mean,
            ..lab_tp()
        };
        // Fixture: a grade pull on each belt totalling half the hold's breached capacity.
        let pull = -0.5 * tp.brake_capacity_n;
        let mut st = fresh(&tp);
        let mut speeds = [0.0f32; 2];
        let mut peak_mean = 0.0f32;
        // Fixture observation budget: two simulated seconds.
        for _ in 0..128 {
            let mut inp = input(0.0, 1.0, speeds, [pull, pull]);
            inp.reaction_slopes = [0.8, 0.3].map(|share| share * fp.inertia / inp.dt);
            speeds = step(&fp, &tp, &mut st, &inp).next_speeds;
            peak_mean = peak_mean.max(((speeds[0] + speeds[1]) * 0.5).abs());
        }
        let difference = (speeds[0] - speeds[1]) * 0.5;
        match mean {
            NeutralMean::Held => {
                assert!(
                    peak_mean < PARK_ENGAGE_SPEED,
                    "a held mean stays at rest on the slope, peaked at {peak_mean} m/s"
                );
                // Resolution: f32 rounding of a sub-metre-per-second landing.
                assert!(
                    (difference - tp.neutral_d_full).abs() < 1e-4,
                    "a held pivot's difference reaches {} m/s, got {difference}",
                    tp.neutral_d_full
                );
            }
            NeutralMean::Free => assert!(
                peak_mean >= PARK_ENGAGE_SPEED,
                "a free mean is set by the grade, peaked at {peak_mean} m/s"
            ),
        }
    }
}

/// The park lever's two brakes carry a neutral stop's mean evenly while the steering member
/// carries the difference, as on equal mobilities. Where the belts' grounds answer unequally, a
/// servo transfer weighted by mobility alone leaves one brake past its static capacity while the
/// other has margin; a tank parked out of a pivot on a steep slope relies on the even share to
/// hold. Fixture: one parking tick of the grade construction, belts creeping apart at a third of
/// the engage speed, unequal reactions and reaction slopes, the even share inside the static
/// capacity and the larger per-side need past it.
#[test]
fn two_parked_brakes_share_a_neutral_stop_evenly() {
    let (fp, tp) = (grade_fixture_fp(), tiger_tp());
    let mut st = TransmissionState {
        park: true,
        ..fresh(&tp)
    };
    let static_cap = tp.brake_capacity_n * tp.brake_static_factor;
    let creep = PARK_ENGAGE_SPEED / 3.0;
    let mut inp = input(
        0.0,
        0.0,
        [creep, -creep],
        [1.15 * static_cap, 0.65 * static_cap],
    );
    inp.reaction_slopes = [0.05, 0.8].map(|share| share * fp.inertia / inp.dt);
    let need = [0, 1].map(|i| {
        inp.reactions[i] - inp.speeds[i] * (fp.inertia + inp.reaction_slopes[i] * inp.dt) / inp.dt
    });
    assert!(
        (need[0] + need[1]) / 2.0 < static_cap && need[0].max(need[1]) > static_cap,
        "the fixture holds only on an even share: {need:?} against {static_cap}"
    );
    let report = step(&fp, &tp, &mut st, &inp);
    assert_eq!(report.held, [true; 2], "a parked brake broke away");
}

/// A held mean is bounded by the parking hold's two-sided capacity, static while the mean is at
/// rest and dynamic once breached (`brake_capacity_for_regime`). A grade between the two holds a
/// resting pivot but keeps a breached one sliding; a grade beyond the static capacity moves a
/// resting one. A pivot on a slope the parked tank cannot hold relies on this to slide too.
#[test]
fn a_held_mean_gives_way_beyond_its_capacity_and_slides_once_breached() {
    let fp = lab_fp();
    let tp = TransmissionParams {
        neutral_mean: NeutralMean::Held,
        ..lab_tp()
    };
    let at_rest = 2.0 * brake_capacity_for_regime(&tp, true, false, 0.0);
    let breached = 2.0 * brake_capacity_for_regime(&tp, true, false, PARK_ENGAGE_SPEED);
    // The final and the peak mean speed over `ticks` of a full-lever pivot under a total grade
    // pull (N).
    let pivot = |pull: f32, mean: f32, ticks: usize| {
        let mut st = fresh(&tp);
        let mut speeds = [mean; 2];
        let side = [-0.5 * pull; 2];
        let mut peak = 0.0f32;
        for _ in 0..ticks {
            speeds = step(&fp, &tp, &mut st, &input(0.0, 1.0, speeds, side)).next_speeds;
            peak = peak.max(((speeds[0] + speeds[1]) * 0.5).abs());
        }
        ((speeds[0] + speeds[1]) * 0.5, peak)
    };
    // Fixture: a pull halfway between the two capacities, observed for one simulated second.
    let between = 0.5 * (at_rest + breached);
    let (_, rest) = pivot(between, 0.0, 64);
    assert!(
        rest < PARK_ENGAGE_SPEED,
        "the static capacity holds a resting mean, peaked at {rest} m/s"
    );
    // One tick from the first breached mean speed.
    let (sliding, _) = pivot(between, PARK_ENGAGE_SPEED, 1);
    assert!(
        sliding > PARK_ENGAGE_SPEED,
        "a breached hold has only its dynamic capacity, so the mean keeps sliding: {sliding} m/s"
    );
    // Fixture: a pull a quarter beyond the static capacity, one tick from rest.
    let (beyond, _) = pivot(1.25 * at_rest, 0.0, 1);
    assert!(
        beyond >= PARK_ENGAGE_SPEED,
        "the hold is bounded: a pull beyond it moves the mean, got {beyond} m/s"
    );
}

/// The L600 constraint converges to the geared ratio: under sustained throttle + tight
/// steer with no ground reaction, d/|m| lands on κ_tight of the active gear.
#[test]
fn l600_constraint_holds_geared_ratio() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    let mut speeds = [0.0f32; 2];
    let mut last = TransmissionReport::default();
    for _ in 0..400 {
        let inp = felt_at_rest(input(0.5, 1.0, speeds, [0.0, 0.0]));
        last = step(&fp, &tp, &mut st, &inp);
        speeds = last.next_speeds;
    }
    assert_eq!(st.steer_step, 2, "|steer| = 1 must engage the tight detent");
    let m = (speeds[0] + speeds[1]) / 2.0;
    let d = (speeds[0] - speeds[1]) / 2.0;
    assert!(m > 0.5, "the tank must be driving (m = {m})");
    let kappa = tp.steer_kappa[(last.gear - 1) as usize].0;
    let ratio = d / m.abs();
    assert!(
        (ratio - kappa).abs() < 0.01 * kappa.max(0.05),
        "d/m = {ratio} must hold κ_tight = {kappa} (gear {})",
        last.gear
    );
}

/// External load can drive belts past the engine's governed speed. The contact law and
/// mechanical over-rev guard rely on receiving that motion instead of a clipped speed;
/// a speed-clamp mutant would discard the impulse this fixture applies during a shift.
#[test]
fn an_external_load_can_drive_belts_past_governed_speed() {
    let (fp, tp) = (lab_fp(), lab_tp());
    for reverse in [false, true] {
        let direction = if reverse { -1.0 } else { 1.0 };
        let speed = direction * 2.0 * tp.governed_speed(reverse);
        let mut state = TransmissionState {
            reverse,
            shift_ticks: 2,
            ..fresh(&tp)
        };
        // FIXTURE DATA: equal downhill reactions increase the signed speed while declutched.
        let reaction = -direction * 10_000.0;
        let report = step(
            &fp,
            &tp,
            &mut state,
            &input(0.0, 0.0, [speed; 2], [reaction; 2]),
        );
        assert!(
            report
                .next_speeds
                .iter()
                .all(|next| direction * next > direction * speed)
        );
    }
}

/// A wide steering detent adds a differential to the mean speed at top-gear cruise.
/// The outer belt must keep that differential, even beyond the ladder's governed mean speed;
/// contact traction relies on the transmission preserving both axes.

#[test]
fn top_gear_wide_steer_preserves_the_outer_belt_differential() {
    let tp = lab_tp();
    let fp = lab_fp();

    let half_tread_m = tp.steer_kappa[0].1 * tp.steer_radii_m[0].1;
    let top = tp.gears_fwd.len() - 1;
    let cruise_m = 0.999 * tp.governed_speed(false);
    let mut st = TransmissionState {
        gear: (top + 1) as u8,
        omega_e: cruise_m * tp.gears_fwd[top] / tp.sprocket_radius,
        ..fresh(&tp)
    };
    let rep = step(
        &fp,
        &tp,
        &mut st,
        &input(1.0, 0.3, [cruise_m, cruise_m], [0.0, 0.0]),
    );

    assert_eq!(rep.steer_step, 1, "0.3 steer must engage the WIDE detent");
    let m = (rep.next_speeds[0] + rep.next_speeds[1]) / 2.0;
    let d = (rep.next_speeds[0] - rep.next_speeds[1]) / 2.0;
    let expected_d = tp.steer_kappa[top].1 * m.abs();
    assert!(
        rep.next_speeds[0] > tp.governed_speed(false),
        "top-gear WIDE outer belt must exceed the {limit} m/s mean-axis limit by its kinematic \
             differential (belts {:?}, m {m}, d {d})",
        rep.next_speeds,
        limit = tp.governed_speed(false),
    );
    let outer_excess = rep.next_speeds[0] - m;
    assert!(
        (d - expected_d).abs() <= 0.02 * expected_d
            && (outer_excess - expected_d).abs() <= 0.02 * expected_d,
        "top-gear WIDE must preserve outer = m + d with d = kappa*m \
             (d {d}, outer excess {outer_excess}, expected {expected_d})"
    );
    let belt_radius = half_tread_m / (d / m.abs());
    assert!(
        (belt_radius - tp.steer_radii_m[top].1).abs() <= 0.02 * tp.steer_radii_m[top].1,
        "top-gear WIDE belt-kinematic radius must stay within 2% of the authored wide radius \
             (got {belt_radius})"
    );
}

/// A tick that carries `m` through zero must not project the
/// constraint onto the pre-tick |m| branch — that enforces `d = s·κ·m` on the wrong
/// side of the cusp, flipping `d` AGAINST the commanded steer for a tick (a yaw
/// impulse, and ringing if m chatters around zero). The scenario: slow forward
/// roll, tight detent, strong equal reactions during a shift interruption — the tick
/// lands m well negative; `d` must stay on the steer's side.
#[test]
fn l600_constraint_survives_m_zero_crossing() {
    let (fp, tp) = (lab_fp(), lab_tp()); // The paid cut and released park isolate steering.
    let mut st = TransmissionState {
        steer_step: 2,
        shift_ticks: 5,
        ..fresh(&tp)
    };
    let (m0, d0) = (0.100f32, 0.043);
    let inp = input(0.5, 1.0, [m0 + d0, m0 - d0], [250_000.0, 250_000.0]);
    let rep = step(&fp, &tp, &mut st, &inp);
    let m_next = (rep.next_speeds[0] + rep.next_speeds[1]) / 2.0;
    let d_next = (rep.next_speeds[0] - rep.next_speeds[1]) / 2.0;
    assert!(
        m_next < 0.0,
        "the scenario must actually cross zero (m {m0} -> {m_next})"
    );
    assert!(
        d_next > -1e-4,
        "positive steer must not produce a flipped (negative) belt difference across \
             the crossing (d {d0} -> {d_next})"
    );
    // And the landing obeys the constraint on the branch it landed on: d = s·κ·|m|.
    let kappa = tp.steer_kappa[0].0;
    assert!(
        (d_next - kappa * m_next.abs()).abs() < 0.02,
        "the re-solved branch must land ON the geared ratio (d {d_next} vs κ|m| {})",
        kappa * m_next.abs()
    );
}

/// Static breakaway and dynamic dissipation are separate capacities. A parked belt inside the
/// multiplied static capacity holds EXACTLY (v̇ = 0, bit-zero); demand past it back-drives.
#[test]
fn brake_capacity_breach_backdrives() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    // Fixture demand between the moving and static capacities, at zero command and speed.
    let r_in = (1.0 + tp.brake_static_factor) / 2.0 * tp.brake_capacity_n;
    let rep = step(
        &fp,
        &tp,
        &mut st,
        &input(0.0, 0.0, [0.0, 0.0], [r_in, r_in]),
    );
    assert_eq!(
        rep.next_speeds,
        [0.0, 0.0],
        "inside static capacity the parked brake holds exactly"
    );
    assert_eq!(
        rep.held, [true; 2],
        "rigid contact consumers need an exact hold verdict"
    );
    let r_out = (tp.brake_static_factor + 1.0) * tp.brake_capacity_n;
    let rep = step(
        &fp,
        &tp,
        &mut st,
        &input(0.0, 0.0, [0.0, 0.0], [r_out, r_out]),
    );
    assert_eq!(
        rep.held, [false; 2],
        "a brake beyond capacity cannot hold a rigid face"
    );
    assert!(
        rep.next_speeds[0] < 0.0 && rep.next_speeds[1] < 0.0,
        "slope demand past static capacity must back-drive the belt (got {:?})",
        rep.next_speeds
    );
}

#[test]
fn static_brake_capacity_requires_every_hold_predicate() {
    let tp = lab_tp();
    let dynamic = tp.brake_capacity_n;
    let static_capacity = dynamic * tp.brake_static_factor;

    assert_eq!(
        brake_capacity_for_regime(&tp, true, false, 0.0),
        static_capacity,
        "a latched belt strictly inside the at-rest band gets static breakaway capacity"
    );
    assert_eq!(
        brake_capacity_for_regime(&tp, false, false, 0.0),
        dynamic,
        "an unlatched settle envelope stays dynamic"
    );
    assert_eq!(
        brake_capacity_for_regime(&tp, true, true, 0.0),
        dynamic,
        "service braking stays dynamic even if stale latch state is present"
    );
    assert_eq!(
        brake_capacity_for_regime(&tp, true, false, PARK_ENGAGE_SPEED),
        dynamic,
        "leaving the strict at-rest band drops the cap that same tick"
    );
}

/// The parking brake settles creep instead of freezing it. Using `B = clamp(R − Q, ±cap)`
/// at a small positive belt speed with `R > Q` would set
/// `v̇ = 0` exactly — positive brake work cancelling grip and drag, preserving creep
/// forever. The stop-force law lands the belt at zero.
#[test]
fn parking_brake_settles_creep() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    // Creep below the latch threshold, zero command, a ground reaction R > Q inside
    // capacity.
    let rep = step(
        &fp,
        &tp,
        &mut st,
        &input(0.0, 0.0, [0.03, 0.03], [20_000.0, 20_000.0]),
    );
    assert!(st.park, "zero command near standstill must latch the park");
    for v in rep.next_speeds {
        assert!(
            v.abs() < 1e-5,
            "the parked brake must settle creep to zero, not hold it (next = {v})"
        );
    }
}

/// A capacity breach retains the lever and its dynamic resistance at every slide speed.
/// Static breakaway capacity applies only while a belt remains in the at-rest band.
#[test]
fn parking_brake_stays_saturated_past_breach() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    let r_breach = 1.7 * tp.brake_capacity_n;
    let mut speeds = [0.0f32; 2];
    let mut last = TransmissionReport::default();
    for _ in 0..30 {
        let inp = input(0.0, 0.0, speeds, [r_breach, r_breach]);
        last = step(&fp, &tp, &mut st, &inp);
        assert_eq!(
            st.clutch,
            ClutchState::Out,
            "the park lever retains neutral past breach"
        );
        speeds = last.next_speeds;
    }
    assert!(
        st.park,
        "the latch must not release without a drive command"
    );
    assert!(
        speeds[0] < -PARK_ENGAGE_SPEED,
        "the breach must back-drive the belt well past the at-rest static regime \
             (speed = {})",
        speeds[0]
    );
    for side in last.forces {
        assert!(
            side >= tp.brake_capacity_n,
            "sliding past the breach, the sprocket force must still carry the full \
                 saturated brake opposing the slide (got {side}, brake capacity {})",
            tp.brake_capacity_n
        );
    }

    // Once moving, the result is bit-identical to a factor-1.0 fixture: the latch persists, but
    // its static multiplier is gone rather than becoming 192 kN/side dynamic braking.
    let moving = input(0.0, 0.0, [-(PARK_ENGAGE_SPEED * 2.0); 2], [r_breach; 2]);
    let mut dynamic_tp = tp.clone();
    dynamic_tp.brake_static_factor = 1.0;
    let mut static_state = TransmissionState {
        park: true,
        ..fresh(&tp)
    };
    let mut dynamic_state = static_state;
    let static_report = step(&fp, &tp, &mut static_state, &moving);
    let dynamic_report = step(&fp, &dynamic_tp, &mut dynamic_state, &moving);
    assert_eq!(
        static_report.forces, dynamic_report.forces,
        "a moving latched slide must drop to dynamic brake capacity"
    );
    assert_eq!(
        static_report.next_speeds, dynamic_report.next_speeds,
        "the post-breach slide must be bit-identical to factor-1.0 dynamic braking"
    );
}

/// Discrete passivity of the stop-force law against free belt integration: the brake's
/// contribution over one tick never pushes the belt PAST the baseline in its direction
/// of motion, never reverses it through zero, and never increases |v_next| beyond the
/// baseline's. Swept over speeds and reactions on both sides of capacity, latched and
/// unlatched.
#[test]
fn brake_is_discretely_passive() {
    let tp = lab_tp();
    let fp = lab_fp();
    for park in [false, true] {
        for v in [-0.6f32, -0.2, -0.03, 0.0, 0.03, 0.2, 0.6] {
            for r in [-1.5f32, -0.5, 0.0, 0.5, 1.5] {
                let r = r * tp.brake_capacity_n;
                let inp = input(0.0, 0.0, [v, v], [r, r]);
                let mut state = TransmissionState { park, ..fresh(&tp) };
                let controls = driver(&construction(&fp), &tp, &mut state, &inp);
                state.park = park;
                let mut q = [0.0; 2];
                let belts = BeltTick::new(&construction(&fp), &controls.input);
                apply_brake_stop_forces(&mut q, &tp, &belts, &state, &controls);
                let free = v - r * inp.dt / fp.inertia;
                for force in q {
                    let (b, f) = (v + (force - r) * inp.dt / fp.inertia, free);
                    assert!(
                        b.abs() <= f.abs() + 1e-4,
                        "park={park} v={v} R={r}: the brake increased belt speed \
                             (braked {b} vs free {f})"
                    );
                    assert!(
                        b * f >= -1e-6,
                        "park={park} v={v} R={r}: the brake pushed the belt through \
                             zero past the free trajectory (braked {b} vs free {f})"
                    );
                }
            }
        }
    }
}

/// Energy honesty over 64-tick windows: Σ(Q_L·v_L + Q_R·v_R)·dt never exceeds the
/// integrated engine power available plus released belt-inertia energy — regeneration
/// recirculates, it does not create (the design's no-free-energy bound). Exercised over
/// a launch, a driving turn, and a pivot — and, for the
/// physical-power split, from an asymmetric rolling start with a hard steer command at gentle
/// throttle (`F_s ≫ F_p`, `m > d > 0`): the case where one SPROCKET's power is negative
/// while both MODAL powers read positive, so the modal split never charged η.
#[test]
fn energy_bound_no_free_energy() {
    let (fp, tp) = (lab_fp(), lab_tp());
    for (throttle, steer, seed) in [
        (1.0, 0.0, [0.0f32, 0.0]),
        (0.7, 0.8, [0.0, 0.0]),
        (0.0, 1.0, [0.0, 0.0]),
        // Steer-dominant at a rolling start — inner sprocket goes negative.
        (0.2, 1.0, [4.0, 2.0]),
        (0.2, -1.0, [4.0, 2.0]),
    ] {
        let mut st = fresh(&tp);
        let mut speeds = seed;
        let dt_s = 1.0_f64 / 64.0;
        for window in 0..6 {
            let mut delivered = 0.0f64;
            let mut available = 0.0f64;
            let e0: f64 = speeds
                .iter()
                .map(|&v| 0.5 * f64::from(fp.inertia) * f64::from(v) * f64::from(v))
                .sum();
            for _ in 0..64 {
                // Synthetic ground reaction: a drag opposing each belt (30 kN/(m/s),
                // saturating at 25 kN) — enough load to exercise the power limiter.
                let reactions = speeds.map(|v| (v * 30_000.0).clamp(-25_000.0, 25_000.0));
                let inp = input(throttle, steer, speeds, reactions);
                let rep = step(&fp, &tp, &mut st, &inp);
                delivered +=
                    f64::from(rep.forces[0] * speeds[0] + rep.forces[1] * speeds[1]) * dt_s;
                available += f64::from(rep.power_available) * dt_s;
                speeds = rep.next_speeds;
            }
            let e1: f64 = speeds
                .iter()
                .map(|&v| 0.5 * f64::from(fp.inertia) * f64::from(v) * f64::from(v))
                .sum();
            let released = (e0 - e1).max(0.0);
            assert!(
                delivered <= available + released + 500.0,
                "t={throttle} s={steer} window {window}: delivered {delivered:.0} J \
                     > available {available:.0} J + released {released:.0} J"
            );
        }
    }
}

/// The game's damage model: recirculation prices physical sprocket work through production
/// motor construction and bisection. Fixture: low transfer efficiency, isolated main clutch,
/// asymmetric rolling belts and a small crank budget force a positive/negative transfer to bind.
#[cfg(feature = "bitprobe")]
#[test]
fn recirculation_splits_physical_output_powers() {
    let (fp, mut tp) = (lab_fp(), lab_tp());
    tp.clutch_capacity = 0.0; // Fixture isolation of the geared steering member.
    tp.recirculation = 0.1; // Fixture loss large enough to bind the work budget.
    let mut st = fresh(&tp);
    st.park = false;
    let inp = input(1.0, 1.0, [5.0, 3.0], [0.0; 2]);
    let rep = step(&fp, &tp, &mut st, &inp);
    assert!(
        rep.power_scale > 0.0 && rep.power_scale < 1.0,
        "{}",
        rep.power_scale
    );
    let physical = [0, 1].map(|i| rep.forces[i] * (inp.speeds[i] + rep.next_speeds[i]) * 0.5);
    assert!(
        physical.iter().any(|p| *p > 0.0) && physical.iter().any(|p| *p < 0.0),
        "{physical:?}"
    );
    for (recorded, actual) in [rep.bitprobe.power_left, rep.bitprobe.power_right]
        .into_iter()
        .zip(physical)
    {
        assert!(
            (recorded - actual).abs() <= actual.abs() * 1e-5,
            "recorded {recorded}, physical {actual}"
        );
    }
    let free = rep.bitprobe.omega_pre + rep.bitprobe.tau_free * inp.dt / tp.engine_inertia;
    let budget = 0.5 * tp.engine_inertia * (free.powi(2) - rep.bitprobe.omega_floor.powi(2));
    let work = recirculated_work(&tp, physical) * inp.dt;
    assert!(
        (work - budget).abs() <= budget * 1e-4,
        "bisection: {work} of {budget} J"
    );
}

/// Coast intent: zero throttle at speed applies the DECLARED
/// compression-braking drag at the CRANK — the rising motoring curve
/// `engine_drag(ω)`, anchored at `drag_fraction × peak` mid-band — and it reaches the
/// belt only through the engaged coupling. With the belt speed HELD constant (this
/// harness feeds fixed speeds), the crank must be steady too, so the clutch transmits
/// the FULL drag torque AT the converged crank speed: the converged per-side force is
/// exactly `engine_drag(ω_converged) × G/r_s / 2` (the steady state is
/// coupling-law-invariant; only the transient shares drag with the crank's inertia).
/// Convergence takes a few ticks: the coupling's per-tick contraction factor is
/// `k²J/(I_m + k²J)` ≈ 0.22 in lab gear 3, plus the first ticks resolve the idle-speed
/// crank against the geared shaft at clutch capacity.
#[test]
fn coast_drag_reaches_belt_through_coupling() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = TransmissionState {
        gear: 3,
        ..fresh(&tp)
    };
    // Mid-band speed for gear 3 (no shift decision interferes).
    let g3 = tp.gears_fwd[2];
    let v = 1_300.0 * RPM_TO_RAD * tp.sprocket_radius / g3;
    let mut rep = TransmissionReport::default();
    for _ in 0..32 {
        rep = step(&fp, &tp, &mut st, &input(0.0, 0.0, [v, v], [0.0, 0.0]));
    }
    assert_eq!(st.gear, 3, "mid-band coast must not shift");
    let expect = -(engine_drag(&tp, st.omega_e) * g3 / tp.sprocket_radius) / 2.0;
    for side in rep.forces {
        assert!(
            (side - expect).abs() < 100.0,
            "converged coasting side force {side} N must be the declared drag share \
                 {expect} N through the coupling"
        );
    }
    // And the crank sits AT the geared speed (locked coast — the readout truth).
    let geared_rpm = v * g3 / tp.sprocket_radius / RPM_TO_RAD;
    assert!(
        (rep.rpm - geared_rpm).abs() < 25.0,
        "locked coast must carry the crank at the geared rpm ({geared_rpm:.0}), got {:.0}",
        rep.rpm
    );
}

/// Motoring drag reads its construction-owned mid-band anchor and grows
/// with speed. The descent law relies on the slope; a flat or incorrectly anchored curve fails.
#[test]
fn motoring_drag_matches_its_anchor_and_grows_with_speed() {
    let tp = tiger_tp();
    let mid = (tp.engine.idle_rpm + tp.engine.governed_rpm) * 0.5;
    let d = |rpm: f32| engine_drag(&tp, rpm * RPM_TO_RAD);
    assert!((d(mid) - tp.peak_torque_nm * tp.drag_fraction).abs() < 0.001);
    assert!(d(tp.engine.idle_rpm) < d(mid));
    assert!(d(mid) < d(tp.engine.governed_rpm));
}

/// The steering member's capacity is per output: each belt carries up to the full datum.
/// A total difference-axis interpretation halves yaw authority. At rest, the neutral servo's
/// exact-landing force is capacity-clamped; this Fixture reaches that per-output clamp.
#[test]
fn pivot_authority_is_per_output_capacity() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let dt = 1.0 / 64.0;
    let mut st = fresh(&tp);
    let rep = step(&fp, &tp, &mut st, &input(0.0, 1.0, [0.0, 0.0], [0.0, 0.0]));
    // The neutral servo's exact-landing force, per-output capacity clamp.
    let expect = (tp.neutral_d_full * fp.inertia / dt).min(tp.steer_capacity_n);
    assert!(
        (rep.forces[0] - expect).abs() < 1.0,
        "left output must carry min(capacity, exact-landing) = {expect}, got {}",
        rep.forces[0]
    );
    assert!(
        (rep.forces[1] + expect).abs() < 1.0,
        "right output mirrors it (counter-rotation), got {}",
        rep.forces[1]
    );
    assert!(
        expect > 0.9 * tp.steer_capacity_n,
        "the lab targets must exercise near-capacity authority ({expect} vs {}) — under the \
         total difference-axis reading the ceiling would be capacity/2",
        tp.steer_capacity_n
    );
}

/// The launch law transmits free torque at its target, rather than clutch-capacity torque.
/// Pricing and the gear law rely on this steady-slip force, in either direction and at part pedal.
#[test]
fn steady_launch_slip_transmits_the_bounded_free_torque() {
    let (fp, mut tp) = (lab_fp(), lab_tp());
    tp.launch_gears = 1; // Fixture: a first-gear launch domain.
    for command in [0.25, 0.5, 1.0] {
        for reverse in [false, true] {
            let mut st = fresh(&tp);
            st.reverse = reverse;
            st.omega_e = tp.launch_omega(command);
            let sign = if reverse { -1.0 } else { 1.0 };
            let [fuel, catch, drag] = engine_torques(&tp, DriveSupply::WHOLE, st.omega_e, command);
            let expected = (fuel + catch - drag).min(tp.clutch_capacity);
            for _ in 0..16 {
                let report = step(
                    &fp,
                    &tp,
                    &mut st,
                    &felt_at_rest(input(sign * command, 0.0, [0.0; 2], [0.0; 2])),
                );
                assert_eq!(st.clutch, ClutchState::Slip);
                let transmitted = sign * report.forces.iter().sum::<f32>() * tp.sprocket_radius
                    / if reverse {
                        tp.gears_rev[0]
                    } else {
                        tp.gears_fwd[0]
                    };
                assert!(
                    (transmitted - expected).abs() < 0.01,
                    "{transmitted} vs {expected}"
                );
                assert!(transmitted <= tp.torque_at(st.omega_e / RPM_TO_RAD) + 0.01);
            }
        }
    }
}

/// The game's driving model bounds a healthy crank under both a loaded launch and a
/// back-driven coast. The clutch's floor cannot create energy or reverse propulsive force: the
/// torque it transmits stays within capacity and never turns backward beyond the crank
/// arithmetic's roundoff, and once the crank has spooled it drives forward. Fixture: first gear, the Driver feeling the hull ride its belts
/// against the fixture's reactions; a full-weight lug at rest (the launch slips) and a 2 m/s
/// back-driven coast (the clutch is in).
#[test]
fn stall_guard_holds_crank_under_grade_lug() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let floor = (tp.engine.idle_rpm - engine_control().idle_guard_band_rpm.value) * RPM_TO_RAD;
    for (throttle, speeds, reactions, regime, label) in [
        (
            1.0f32,
            [0.0f32, 0.0],
            [200_000.0f32, 200_000.0],
            ClutchState::Slip,
            "full-W lug",
        ),
        (
            0.0,
            [-2.0, -2.0],
            [-40_000.0, -40_000.0],
            ClutchState::In,
            "coast backslide",
        ),
    ] {
        let mut st = fresh(&tp);
        let mut inp = input(throttle, 0.0, speeds, reactions);
        // Fixture: ground that grips beyond any gear's pull.
        inp.sensed = Some(Sensed {
            speed: speeds[0],
            resistance: reactions[0] + reactions[1],
            push_difference: reactions[0] - reactions[1],
            drive: reactions[0] + reactions[1],
            drive_gain: 0.0,
            grip: Some(f32::INFINITY),
            stall: None,
            failed: None,
            stalled: false,
            observed: true,
        });
        #[cfg(feature = "bitprobe")]
        let (roundoff, mut settled) = (
            DriverResolution::load().crank_roundoff_share * tp.roundoff_torque_scale(),
            0.0,
        );
        for tick in 0..128 {
            let rep = step(&fp, &tp, &mut st, &inp);
            assert_eq!(
                st.gear, 1,
                "{label} tick {tick}: the fixture's first gear holds"
            );
            assert_eq!(st.clutch, regime, "{label} tick {tick}");
            assert!(
                st.omega_e >= floor - 1e-3,
                "{label} tick {tick}: ω_e {:.0} rpm fell below the stall-guard floor \
                     ({:.0} rpm)",
                st.omega_e / RPM_TO_RAD,
                floor / RPM_TO_RAD
            );
            #[cfg(feature = "bitprobe")]
            {
                let torque = rep.bitprobe.tau_c * rep.power_scale;
                assert!(
                    (-roundoff..=tp.clutch_capacity).contains(&torque),
                    "{label} tick {tick}: clutch torque {torque} N m outside [0, capacity]"
                );
                settled = torque;
            }
            assert!(
                rep.forces[0] >= 0.0 && rep.forces[1] >= 0.0,
                "{label} tick {tick}: the slipping clutch must keep delivering \
                     nonnegative drive while the crank spools (forces {:?})",
                rep.forces
            );
        }
        #[cfg(feature = "bitprobe")]
        assert!(
            settled > roundoff,
            "{label}: the settled clutch must transmit forward torque"
        );
    }
}

/// Rev-match across an upshift — the crank is CONTINUOUS through the window
/// (no teleport: per-tick slew bounded by `(capacity + τ_free)/J·dt` ≈ 189 rpm/tick in
/// the lab), lands within a bounded gap of the new geared speed at window end (drag-only
/// shedding covers ≈ 410 of the ≈ 660 rpm step in the 0.31 s window; the clutch
/// shoulders the ≈ 250 rpm residual at capacity for a few ticks — the bounded physical
/// cost of the shift), and re-locks to the geared point within a handful of engaged
/// ticks.
#[test]
fn rev_match_across_upshift_is_continuous() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let g1 = tp.gears_fwd[0];
    let g2 = tp.gears_fwd[1];
    let v_warm = 1_600.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
    let v_up = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
    let mut st = fresh(&tp);
    // Warm to the locked geared point below the up band.
    for _ in 0..32 {
        step(
            &fp,
            &tp,
            &mut st,
            &input(1.0, 0.0, [v_warm, v_warm], [0.0, 0.0]),
        );
    }
    st.gear = 2;
    st.shift_ticks = tp.shift_ticks.get();
    let target_rpm = v_up * g2 / tp.sprocket_radius / RPM_TO_RAD; // ≈ 1121
    let mut prev_rpm = st.omega_e / RPM_TO_RAD;
    let mut window_end_gap = None;
    let mut ticks_since_window = 0u32;
    let mut rep = TransmissionReport::default();
    for tick in 0..96 {
        rep = step(
            &fp,
            &tp,
            &mut st,
            &input(1.0, 0.0, [v_up, v_up], [0.0, 0.0]),
        );
        let rpm = st.omega_e / RPM_TO_RAD;
        assert!(
            (rpm - prev_rpm).abs() <= 250.0,
            "tick {tick}: crank teleported {prev_rpm:.0} -> {rpm:.0} rpm"
        );
        prev_rpm = rpm;
        if st.gear == 2 && !rep.shifting && window_end_gap.is_none() {
            window_end_gap = Some((rpm - target_rpm).abs());
        }
        if window_end_gap.is_some() {
            ticks_since_window += 1;
        }
    }
    assert_eq!(st.gear, 2, "the upshift must have committed");
    let gap = window_end_gap.expect("the window must end inside the run");
    assert!(
        gap <= 400.0,
        "rpm at window end must be within 400 rpm of the geared landing \
             ({target_rpm:.0}), gap {gap:.0}"
    );
    assert!(
        ticks_since_window > 16,
        "post-window settling must be observed"
    );
    // Re-lock anchor: the geared rpm of the belt the transmission itself integrated
    // (this harness holds the INPUT speeds, so the lock's fixed point rides
    // `k·τ_free·dt/I_m` above the held value — the crank follows THAT belt exactly).
    let m_next = (rep.next_speeds[0] + rep.next_speeds[1]) / 2.0;
    let lock_rpm = m_next * g2 / tp.sprocket_radius / RPM_TO_RAD;
    let final_rpm = st.omega_e / RPM_TO_RAD;
    assert!(
        (final_rpm - lock_rpm).abs() < 50.0,
        "the engaged clutch must re-lock the crank to the geared point of the \
             integrated belt ({lock_rpm:.0}), got {final_rpm:.0}"
    );
}

/// Unloaded free-rev — declutched full steer at standstill (the pivot's crank
/// demand) revs the crank from idle toward the steer-demand target (the PEAK-TORQUE
/// rpm, reached dynamically; deliberately NOT the
/// governed cut-out, where `torque_at·ω = 0` would zero the pivot's power budget).
/// Lab arithmetic: Δω = 500 rpm = 52.4 rad/s at ≈ τ/J ≈ 2000/4 = 500 rad/s² plus the
/// proportional-band taper → ≈ 0.15–0.3 s to 95%; the steady point parks ≈ 30 rpm
/// under the target where the taper's fueling meets the re-engaging drag. Pinned with
/// margin.
#[test]
fn free_rev_reaches_steer_target_promptly() {
    let (fp, mut tp) = (lab_fp(), lab_tp());
    // Fixture isolation: steer requests crank rpm but transmits no work to the belts.
    tp.steer_capacity_n = 0.0;
    let mut st = fresh(&tp);
    let mut reached = None;
    for tick in 0..128 {
        step(&fp, &tp, &mut st, &input(0.0, 1.0, [0.0, 0.0], [0.0, 0.0]));
        let rpm = st.omega_e / RPM_TO_RAD;
        if reached.is_none() && rpm >= 0.95 * tp.peak_torque_rpm {
            reached = Some(tick + 1);
        }
    }
    let ticks = reached.expect("the crank must reach 95% of the steer target in 2 s");
    let secs = ticks as f32 / TICK_HZ;
    println!("lab free-rev idle -> 95% of peak-torque rpm: {secs:.3} s");
    assert!(
        (0.05..=0.6).contains(&secs),
        "free-rev time {secs:.3} s outside the pinned band"
    );
    let steady = st.omega_e / RPM_TO_RAD;
    assert!(
        (tp.peak_torque_rpm - 150.0..=tp.peak_torque_rpm + 50.0).contains(&steady),
        "the declutched full-steer crank must park at the peak-torque operating point \
             (~{:.0} rpm), got {steady:.0} — a cut-out park would zero pivot power",
        tp.peak_torque_rpm
    );
}

/// Service braking cannot snap the crank beyond clutch capacity. Feasibility uses the final
/// belt state: a pre-brake `exact` decision could admit a snap that stop forces make infeasible.
/// DERIVED Fixture counterexample: 120 kN / 8 t / 64 Hz drops the belt by about 0.23 m/s per
/// tick; snapping the crank by about 20 rad/s would imply
/// `τ_free − Δω·J/dt ≈ −550 − 20·4·64 ≈ −5.7 kN·m` through a 2.86 kN·m clutch.
/// Each tick must instead obey `|Δω|·J/dt ≤ capacity + |τ_free|`; `engine_drag` supplies the
/// warmed crank's motoring torque.
#[test]
fn braking_never_teleports_crank_past_clutch_capacity() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let dt = 1.0 / 64.0;
    let mut st = fresh(&tp);
    // Warm to a locked coast in gear 1 at m = 1.0 (held speeds).
    for _ in 0..32 {
        step(&fp, &tp, &mut st, &input(0.0, 0.0, [1.0, 1.0], [0.0, 0.0]));
    }
    assert!(
        st.omega_e / RPM_TO_RAD > tp.engine.idle_rpm,
        "the warm-up must have locked the crank above idle"
    );
    // Full opposing throttle, CLOSED loop: the brake-driven belt drop must not drag
    // the crank faster than the clutch can physically pull it. The drag bound is the
    // rising motoring curve AT the warmed crank speed — the crank only falls from here
    // and the curve rises with ω, so this is the run's maximum |τ_free|.
    let drag_max = engine_drag(&tp, st.omega_e);
    let slew_bound = (tp.clutch_capacity + drag_max) * dt / tp.engine_inertia + 0.1;
    let mut speeds = [1.0f32, 1.0];
    for tick in 0..64 {
        let m_pre = (speeds[0] + speeds[1]) / 2.0;
        if m_pre < 0.5 {
            break; // swap/declutch territory — the teleport window is over.
        }
        let omega_pre = st.omega_e;
        let rep = step(&fp, &tp, &mut st, &input(-1.0, 0.0, speeds, [0.0, 0.0]));
        let delta = st.omega_e - omega_pre;
        assert!(
            delta >= -slew_bound,
            "tick {tick}: braking dragged the crank {delta:.1} rad/s in one tick — \
                 an implied clutch torque past capacity (bound {slew_bound:.1} rad/s); \
                 the infeasible snap is back"
        );
        speeds = rep.next_speeds;
    }
    let floor = (tp.engine.idle_rpm - engine_control().idle_guard_band_rpm.value) * RPM_TO_RAD;
    assert!(
        st.omega_e >= floor,
        "the crank must end above the hard floor"
    );
}

/// The lock torque that slows the crank is the torque the belts receive: in a geared turn inside
/// clutch and steering capacity, the crank's own integration lands on the belts' mean, so the
/// re-anchor moves nothing. A lock solved on the untied mean lands above it, and the re-anchor
/// then discards crank work no sprocket received; the engine's work budget relies on this.
/// Fixture: second gear, tight detent, three quarters of governed speed, the difference off its
/// geared value, unequal reactions and unequal reaction slopes.
#[cfg(feature = "bitprobe")]
#[test]
fn a_geared_turn_integrates_the_crank_onto_the_steered_belts() {
    for (fp, tp) in class_drivetrains() {
        let k = tp.gears_fwd[1] / tp.sprocket_radius;
        let m = 0.75 * tp.engine.governed_rpm * RPM_TO_RAD / k;
        let d = 0.8 * tp.steer_kappa[1].0 * m;
        let mut st = TransmissionState {
            gear: 2,
            steer_step: 2,
            clutch: ClutchState::In,
            omega_e: k * m,
            ..fresh(&tp)
        };
        let weight = fp.grip.side_weight;
        let mut inp = input(1.0, 1.0, [m + d, m - d], [0.2 * weight, -0.1 * weight]);
        inp.reaction_slopes = [0.8, 0.3].map(|share| share * fp.inertia / inp.dt);
        let probe = step(&fp, &tp, &mut st, &inp).bitprobe;
        assert_eq!(probe.j[0] - probe.j[1], 1.0, "the geared constraint acts");
        assert!(probe.reanchor_feasible, "the lock is inside capacity");
        assert!(
            (probe.omega_integrated - probe.reanchor_locked).abs() <= 1e-4 * probe.reanchor_locked,
            "the crank integrated to {} rad/s against belts locked at {}",
            probe.omega_integrated,
            probe.reanchor_locked
        );
    }
}

/// A belt solves its ground reaction implicitly: on a ground stiff enough that the explicit belt
/// update reverses its step every tick, the coupled update does not. The grip law's shear length
/// relies on this to be a ground fact rather than the fixed step's stability bound.
/// Fixture: one belt of the grade construction pushing at half its grip, from rest, into an
/// elastic share over a 10 mm shear length with the grip law's damper.
#[test]
fn the_coupled_belt_settles_where_the_explicit_one_alternates() {
    let fp = grade_fixture_fp();
    let dt = 1.0 / TICK_HZ;
    let grip = fp.ground_grip * fp.grip.side_weight;
    let shear = 0.010;
    let damper = 2.0 * forces::GRIP_ELEMENT_DAMPING_RATIO
        / (shear * fp.ground_grip * super::super::derive::G).sqrt();
    let reversals = |coupled: bool| {
        let (mut speed, mut strain) = (0.0_f32, 0.0_f32);
        let mut steps = Vec::new();
        for _ in 0..TICK_HZ as usize {
            strain = (strain + speed * dt).clamp(-shear, shear);
            let share = strain / shear + speed * damper;
            let (reaction, slope) = if share.abs() < 1.0 {
                (grip * share, grip * (dt / shear + damper))
            } else {
                (grip * share.signum(), 0.0)
            };
            let mut inp = input(0.0, 0.0, [speed; 2], [reaction; 2]);
            inp.reaction_slopes = [if coupled { slope } else { 0.0 }; 2];
            let next = BeltTick::new(&construction(&fp), &inp).land(0, 0.5 * grip);
            steps.push(next - speed);
            speed = next;
        }
        steps
            .windows(3)
            .filter(|w| w[0] * w[1] < 0.0 && w[1] * w[2] < 0.0)
            .count()
    };
    assert!(
        reversals(false) > 0,
        "the fixture ground is one the explicit update cannot resolve"
    );
    assert_eq!(reversals(true), 0, "the coupled belt alternated");
}

/// Driver state, not belt-speed hysteresis, controls clutch regimes. A launch slips above idle
/// until its shaft reaches its target, and a shift always interrupts mean-axis drive. Fixture: belts
/// run free of the ground under a hull at rest.
#[test]
fn clutch_states_cover_rest_launch_lock_and_shift() {
    let (fp, mut tp) = (lab_fp(), lab_tp());
    tp.launch_gears = 1; // Fixture: a first-gear launch domain.
    let mut st = fresh(&tp);
    step(&fp, &tp, &mut st, &input(0.0, 0.0, [0.0; 2], [0.0; 2]));
    assert_eq!(st.clutch, ClutchState::Out);
    // Fixture: a neutral pivot's unequal belt speeds still require the main clutch Out.
    step(&fp, &tp, &mut st, &input(0.0, 1.0, [0.3, -0.05], [0.0; 2]));
    assert_eq!(st.clutch, ClutchState::Out);
    let launch = tp.launch_omega(0.5);
    st.omega_e = launch;
    for omega in [0.0, (tp.engine.idle_rpm * RPM_TO_RAD + launch) / 2.0] {
        let speed = omega * tp.sprocket_radius / tp.gears_fwd[0];
        step(
            &fp,
            &tp,
            &mut st,
            &felt_free_at_rest(input(0.5, 0.0, [speed; 2], [0.0; 2])),
        );
        assert_eq!(st.clutch, ClutchState::Slip);
    }
    let speed = launch * tp.sprocket_radius / tp.gears_fwd[0];
    step(
        &fp,
        &tp,
        &mut st,
        &felt_free_at_rest(input(0.5, 0.0, [speed; 2], [0.0; 2])),
    );
    assert_eq!(st.clutch, ClutchState::In);
    st.shift_ticks = 2;
    step(
        &fp,
        &tp,
        &mut st,
        &felt_free_at_rest(input(0.5, 0.0, [speed; 2], [0.0; 2])),
    );
    assert_eq!(st.clutch, ClutchState::Out);
}

/// An individually finite authoring cannot publish unusable derived ratios. The live coupling
/// and Driver rely on this load gate instead of encountering infinities during play.
#[test]
fn derived_parameter_overflow_is_refused_at_construction() {
    let mut authoring = tiger_authoring();
    authoring.sprocket_radius_m = f32::MAX;
    assert!(
        TransmissionParams::from_authoring(&authoring)
            .unwrap_err()
            .to_string()
            .contains("derived")
    );
    let mut authoring = tiger_authoring();
    authoring.half_tread_m = f32::MIN_POSITIVE;
    let radii = vec![(f32::MAX, f32::MAX); authoring.forward_speeds_kmh.len()];
    authoring.steer_radii_m = &radii;
    assert!(
        TransmissionParams::from_authoring(&authoring)
            .unwrap_err()
            .to_string()
            .contains("derived steering curvature")
    );
}

/// The game's damage model: under a full command the clutch latches at any positive combustion
/// share (intent thresholds read the command, never the share), and a half engine pulls away
/// more weakly than a whole one. Full reverse while rolling brakes at full service capacity
/// through `step` whatever the engine supplies: brakes are construction. A step that scales the
/// service brakes, or a command threshold that turns a small share into no drive, fails here.
/// Fixture pools are 30, 168 and 336 of 336 HP; the belts run free of ground reactions.
#[test]
fn reduced_supply_pulls_away_and_retains_full_service_brakes() {
    let fp = lab_fp();
    let tp = lab_tp();
    let reduced_engine = DriveSupply {
        engine_condition: 30.0 / 336.0,
        task_rate: 30.0 / 336.0,
        ..DriveSupply::WHOLE
    };
    let speeds = [1.0, 0.5, reduced_engine.engine_condition].map(|combustion| {
        let mut state = fresh(&tp);
        let mut speeds = [0.0; 2];
        for _ in 0..64 {
            let mut full = felt_at_rest(input(1.0, 0.0, speeds, [0.0; 2]));
            full.supply.engine_condition = combustion;
            full.supply.task_rate = combustion;
            let report = step(&fp, &tp, &mut state, &full);
            speeds = report.next_speeds;
            assert!(
                state.clutch != ClutchState::Out || report.shifting,
                "share {combustion}: the clutch transmits outside shift windows"
            );
        }
        speeds[0]
    });
    assert!(
        0.0 < speeds[1] && speeds[1] < speeds[0],
        "a half engine pulls away weakly: {speeds:?}"
    );

    let rolling = TransmissionState {
        park: false,
        ..fresh(&tp)
    };
    let mut reverse = input(-1.0, 0.0, [10.0; 2], [0.0; 2]);
    let (mut whole_state, mut braked_state) = (rolling, rolling);
    let whole = step(&fp, &tp, &mut whole_state, &reverse);
    reverse.supply = reduced_engine;
    let braked = step(&fp, &tp, &mut braked_state, &reverse);
    assert_eq!(
        braked.forces, whole.forces,
        "at idle with no fueling, a reduced engine changes nothing the brakes or drag do"
    );
    assert!(
        braked
            .forces
            .iter()
            .all(|force| *force <= -tp.brake_capacity_n),
        "full service brakes plus engine braking: {:?}",
        braked.forces
    );

    let mut no_transmission = input(1.0, 1.0, [0.0; 2], [0.0; 2]);
    no_transmission.supply = DriveSupply {
        transmission_condition: 0.0,
        ..DriveSupply::WHOLE
    };
    let mut state = fresh(&tp);
    let report = step(&fp, &tp, &mut state, &no_transmission);
    assert_eq!(
        report.forces, [0.0; 2],
        "a transmission out of action transmits nothing"
    );
    assert_eq!(report.next_speeds, [0.0; 2]);
    assert!(state.omega_e.is_finite());
}

/// The game's damage model: the exact at-rest input must not create belt kinetic energy.
#[test]
fn no_combustion_no_stored_energy_no_pivot_work() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    st.omega_e = 0.0;
    let mut inp = input(0.0, 1.0, [0.0; 2], [0.0; 2]);
    inp.supply.engine_condition = 0.0;
    inp.supply.task_rate = 0.0;
    let rep = step(&fp, &tp, &mut st, &inp);
    assert_eq!(rep.next_speeds, [0.0; 2]);
}

/// The game's damage model: the crank and Driver price the same reduced full-pedal
/// torque, including construction drag that does not scale with combustion.
#[test]
fn drag_release_and_driver_read_the_command() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    st.park = false;
    st.omega_e = 1550.0 * RPM_TO_RAD; // Fixture crank operating point above idle.
    let ratio = tp.gears_fwd[0];
    let speed = st.omega_e * tp.sprocket_radius / ratio;
    let mut inp = input(1.0, 0.0, [speed; 2], [0.0; 2]);
    inp.supply.engine_condition = 0.25;
    inp.supply.task_rate = 0.25;
    let rep = step(&fp, &tp, &mut st, &inp);
    let modeled = price_driver_force(&tp, inp.supply, speed, ratio, 1.0, ClutchState::In);
    let [induced, idle, drag] = engine_torques(&tp, inp.supply, 1550.0 * RPM_TO_RAD, 1.0);
    let actual =
        (induced + idle - drag).min(tp.clutch_capacity * inp.supply.transmission_condition) * ratio
            / tp.sprocket_radius;
    assert!(
        (modeled - actual).abs() < 0.01,
        "Driver price {modeled}, crank {actual}"
    );
    assert!(rep.forces[0].is_finite());
    #[cfg(feature = "bitprobe")]
    {
        assert_eq!(rep.bitprobe.tau_drag, engine_drag(&tp, 1550.0 * RPM_TO_RAD));
        assert!((rep.bitprobe.tau_free - (induced + idle - drag)).abs() < 1e-3);
    }
}

/// The game's damage model: a zero-capacity fixture clutch isolates service brakes from
/// engine braking, so each supply dimension gets an independent mutant check.
fn assert_full_service_brakes(supply: DriveSupply) {
    let fp = lab_fp();
    let mut tp = lab_tp();
    tp.clutch_capacity = 0.0; // Fixture isolation: no main-clutch torque.
    let mut st = fresh(&tp);
    st.park = false;
    let mut inp = input(-1.0, 0.0, [10.0; 2], [0.0; 2]); // Fixture rolling state.
    inp.supply = supply;
    assert_eq!(
        step(&fp, &tp, &mut st, &inp).forces,
        [-tp.brake_capacity_n; 2]
    );
}

#[test]
fn full_service_brakes_ignore_competence() {
    assert_full_service_brakes(DriveSupply {
        task_rate: 0.6,
        ..DriveSupply::WHOLE
    });
}

#[test]
fn full_service_brakes_ignore_combustion() {
    assert_full_service_brakes(DriveSupply {
        engine_condition: 0.25,
        task_rate: 0.25,
        ..DriveSupply::WHOLE
    });
}

#[test]
fn full_service_brakes_ignore_transmitted_capacity() {
    assert_full_service_brakes(DriveSupply {
        transmission_condition: 0.0,
        ..DriveSupply::WHOLE
    });
}

/// The game's damage model: losing transmitted capacity changes a locked coupling on this
/// tick; crank inertia cannot smooth a disconnected clutch into continued belt force.
#[test]
fn supply_loss_reaches_a_locked_clutch_on_its_tick() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    st.park = false;
    st.omega_e = 1550.0 * RPM_TO_RAD; // Fixture synchronous operating point.
    let speed = st.omega_e * tp.sprocket_radius / tp.gears_fwd[0];
    let mut inp = input(1.0, 0.0, [speed; 2], [0.0; 2]);
    let mut whole_state = st;
    let whole = step(&fp, &tp, &mut whole_state, &inp);
    #[cfg(feature = "bitprobe")]
    assert!(
        whole.bitprobe.engaged && whole.bitprobe.reanchor_feasible,
        "the fixture must start with a locking clutch"
    );
    let mut reduced_engine = input(1.0, 0.0, [speed; 2], [0.0; 2]);
    reduced_engine.supply.engine_condition = 0.0;
    reduced_engine.supply.task_rate = 0.0;
    let no_combustion = step(&fp, &tp, &mut st, &reduced_engine);
    assert!(
        no_combustion.forces[0] < whole.forces[0],
        "combustion torque reaches a locked clutch on its tick"
    );
    println!(
        "MEASURED locked-clutch source step, force per belt: whole {}, engine empty {} N",
        whole.forces[0], no_combustion.forces[0]
    );
    let mut st = TransmissionState {
        omega_e: 1550.0 * RPM_TO_RAD,
        park: false,
        ..fresh(&tp)
    };
    inp.supply.transmission_condition = 0.0;
    let lost = step(&fp, &tp, &mut st, &inp);
    assert!(whole.forces[0] > 0.0);
    assert_eq!(lost.forces, [0.0; 2]);
}

/// The game's damage model: without an occupant, rest cannot set the park lever; a lever already set keeps holding. This must hold at the transmission seam.
#[test]
fn an_unmanned_station_preserves_the_park_lever() {
    let (fp, tp) = (lab_fp(), lab_tp());
    for parked in [false, true] {
        let mut st = fresh(&tp);
        st.park = parked;
        st.omega_e = 0.0; // Fixture: no stored crank energy obscures the brake decision.
        let mut inp = input(0.0, 0.0, [0.0; 2], [1000.0; 2]); // Fixture back-driving load.
        inp.supply.control_authority = false;

        inp.supply.task_rate = 0.0;
        let rep = step(&fp, &tp, &mut st, &inp);
        assert_eq!(st.park, parked);
        if parked {
            assert_eq!(rep.next_speeds, [0.0; 2]);
        } else {
            assert!(rep.next_speeds.iter().all(|speed| *speed < 0.0));
        }
    }
}

/// The game's driving model: above the Driver's rev target a pivot has no combustion.
/// Steering must spend stored crank energy once; total belt-plus-crank energy cannot grow.
#[test]
fn a_pivot_without_combustion_spends_only_stored_crank_energy() {
    let (fp, tp) = (lab_fp(), lab_tp());
    for rpm in [0.0, tp.engine.governed_rpm, tp.engine.crank_limit_rpm] {
        // Fixture initial energy states.
        let mut st = fresh(&tp);
        st.omega_e = rpm * RPM_TO_RAD;
        let mut speeds = [0.0; 2];
        let energy = |omega: f32, speeds: [f32; 2]| {
            0.5 * tp.engine_inertia * omega * omega
                + speeds
                    .into_iter()
                    .map(|v| 0.5 * fp.inertia * v * v)
                    .sum::<f32>()
        };
        let mut previous = energy(st.omega_e, speeds);
        let mut pivoted = false;
        for _ in 0..64 * 5 {
            if rpm > 0.0 && st.omega_e < tp.peak_torque_rpm * RPM_TO_RAD {
                break; // Below the rev target the Driver may fuel again.
            }
            let mut inp = input(0.0, 1.0, speeds, [0.0; 2]);
            if rpm == 0.0 {
                inp.supply.task_rate = 0.0; // A stopped, unworked control is the zero-energy corner.
            }
            let rep = step(&fp, &tp, &mut st, &inp);
            #[cfg(feature = "bitprobe")]
            assert_eq!(rep.bitprobe.tau_induced + rep.bitprobe.tau_idle, 0.0);
            speeds = rep.next_speeds;
            pivoted |= speeds[0] > PARK_ENGAGE_SPEED && speeds[1] < -PARK_ENGAGE_SPEED;
            if pivoted {
                assert!(!st.park);
                assert_eq!(st.clutch, ClutchState::Out);
            }
            let next = energy(st.omega_e, speeds);
            // Resolution: f32 kinetic-energy arithmetic permits a relative roundoff margin.
            assert!(
                next <= previous * (1.0 + 1e-5) + 1e-5,
                "no source: energy rose from {previous} to {next} J"
            );
            previous = next;
        }
        assert_eq!(
            pivoted,
            rpm > 0.0,
            "every stored-energy seed must actually pivot"
        );
    }
}

/// Recirculation losses apply only to an inner-to-outer transfer; a pure overrun keeps the
/// declared engine drag. The admitted zero-efficiency corner cannot turn zero output into NaN.
#[test]
fn pure_engine_braking_does_not_pay_an_inner_to_outer_transfer_loss() {
    let mut tp = lab_tp();
    for efficiency in [0.0, 0.5, 1.0] {
        // Fixture efficiency corners.
        tp.recirculation = efficiency;
        assert_eq!(recirculated_work(&tp, [-20.0, -30.0]), -50.0);
        assert_eq!(recirculated_work(&tp, [0.0; 2]), 0.0);
        assert_eq!(recirculated_work(&tp, [20.0, 30.0]), 50.0);
    }
}

/// The game's damage model: turning and propulsion cannot spend an empty engine's initial
/// kinetic energy twice, including an already-rolling asymmetric pair of belts.
#[test]
fn all_drive_modes_without_combustion_conserve_total_kinetic_energy() {
    let (fp, tp) = (lab_fp(), lab_tp());
    for (throttle, steer) in [(1.0, 0.0), (0.7, 0.8), (0.0, 1.0)] {
        for seed in [[0.0; 2], [0.5, 0.25], [5.0, -3.0]] {
            // Fixture initial velocities.
            let mut st = fresh(&tp);
            let mut speeds = seed;
            let energy = |omega: f32, speeds: [f32; 2]| {
                0.5 * tp.engine_inertia * omega.powi(2)
                    + 0.5 * fp.inertia * (speeds[0].powi(2) + speeds[1].powi(2))
            };
            let mut previous = energy(st.omega_e, speeds);
            for _ in 0..64 {
                let mut inp = input(throttle, steer, speeds, [0.0; 2]);
                inp.supply.engine_condition = 0.0;
                inp.supply.task_rate = 0.0;
                speeds = step(&fp, &tp, &mut st, &inp).next_speeds;
                let next = energy(st.omega_e, speeds);
                // Resolution: relative f32 roundoff, not an energy source.
                assert!(
                    next <= previous * (1.0 + 1e-5) + 1e-5,
                    "mode {throttle}/{steer}, seed {seed:?}: {previous} -> {next} J"
                );
                previous = next;
            }
        }
    }
}

/// The game's damage model: an emptied declutched engine stops through its authored
/// construction drag and inertia, rather than an idle-guard-shaped exponential tail.
#[test]
fn emptied_declutched_engine_runs_down_under_construction_drag() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    st.omega_e = 500.0 * RPM_TO_RAD; // Fixture start below healthy idle.
    let mut inp = input(0.0, 0.0, [0.0; 2], [0.0; 2]);
    inp.supply.engine_condition = 0.0;
    inp.supply.task_rate = 0.0;
    inp.supply.transmission_condition = 0.0; // Fixture: no external crank work.
    let start = st.omega_e;
    let base_drag =
        tp.peak_torque_nm * tp.drag_fraction * engine_control().motoring_drag_base_share.value;
    let bound = tp.engine_inertia * start / base_drag;
    let mut ticks = 0;
    while st.omega_e > 0.0 && ticks < 64 * 10 {
        step(&fp, &tp, &mut st, &inp);
        ticks += 1;
    }
    let elapsed = ticks as f32 * inp.dt;
    println!("MEASURED empty declutched crank: {elapsed} s; DERIVED Coulomb upper bound {bound} s");
    assert_eq!(st.omega_e, 0.0);
    assert!(elapsed <= bound + inp.dt);
}

/// The game's damage model: the full-load net curve already includes construction losses.
/// Losing combustion cannot release those losses through a held pedal.
#[test]
fn absent_combustion_keeps_drag_at_full_pedal() {
    let tp = lab_tp();
    let supply = DriveSupply {
        engine_condition: 0.0,
        task_rate: 0.0,
        ..DriveSupply::WHOLE
    };
    let omega = 1550.0 * RPM_TO_RAD; // Fixture operating point.
    let held = engine_torques(&tp, supply, omega, 1.0);
    let released = engine_torques(&tp, supply, omega, 0.0);
    assert!(held[2] > 0.0);
    assert_eq!(held, released);
}

/// The full-load curve remains attainable at full pedal; releasing fuel above idle retains
/// construction drag. A zero-torque replacement could pass the upper-bound test but fails here.
#[test]
fn full_pedal_reaches_the_curve_and_release_retains_drag() {
    let tp = lab_tp();
    for rpm in [
        tp.engine.idle_rpm,
        tp.peak_torque_rpm,
        tp.engine.governed_rpm,
    ] {
        let [fuel, catch, drag] = engine_torques(&tp, DriveSupply::WHOLE, rpm * RPM_TO_RAD, 1.0);
        assert!((fuel + catch - drag - tp.torque_at(rpm)).abs() < 1e-3);
    }
    let [fuel, catch, drag] = engine_torques(
        &tp,
        DriveSupply::WHOLE,
        tp.peak_torque_rpm * RPM_TO_RAD,
        0.0,
    );
    assert_eq!(fuel + catch, 0.0);
    assert!(drag > 0.0);
}

/// The game's damage model: commanded work consumes one task rate even when
/// local module condition differs. Fixture: a reduced source and a further task bottleneck.
#[test]
fn commanded_work_consumes_the_task_rate_once() {
    let tp = lab_tp();
    let supply = DriveSupply {
        task_rate: 0.3,
        engine_condition: 0.5,
        transmission_condition: 0.75,
        ..DriveSupply::WHOLE
    };
    let omega = 1550.0 * RPM_TO_RAD;
    let [whole, ..] = engine_torques(&tp, DriveSupply::WHOLE, omega, 1.0);
    let [applied, ..] = engine_torques(&tp, supply, omega, 1.0);
    assert!((applied / whole - supply.task_rate).abs() < 1e-6);
}

/// The game's driving and damage model: without task work, clutch opening cannot persist at
/// parked rest or launch, or begin during motion. A rate gate on the park lever fails the rest arms.
#[test]
fn zero_task_rate_springs_clutch_in_at_rest_and_in_motion() {
    for tp in [lab_tp(), tiger_tp()] {
        // Fixture: moving drive, parked release, stopped launch and unparked release.
        for (park, command, speed) in [
            (false, 1.0, 5.0),
            (true, 0.0, 0.0),
            (false, 1.0, 0.0),
            (false, 0.0, 0.0),
        ] {
            let mut state = TransmissionState {
                park,
                clutch: ClutchState::Out,
                ..fresh(&tp)
            };
            let mut inp = input(command, 0.0, [speed; 2], [0.0; 2]);
            inp.supply.task_rate = 0.0;
            driver(&construction(&lab_fp()), &tp, &mut state, &inp);
            assert_eq!(
                state.clutch,
                ClutchState::In,
                "park={park} command={command} speed={speed}"
            );
        }
    }
}

/// The game's driving and damage model: one task bottleneck lowers combustion, even when
/// only transmission condition is reduced. Mechanical capacity independently limits coupling.
#[test]
fn transmission_bottleneck_reduces_the_one_drive_task_rate() {
    use crate::damage::{Group, Part};
    let tp = lab_tp();
    // Fixture: independent mandatory sources, whole engine and a half transmission.
    let task_rate = crate::damage::evaluate(
        &vec![
            Group::Single(Part::Engine),
            Group::Single(Part::Transmission),
        ],
        &[(Part::Engine, 1.0), (Part::Transmission, 0.5)].into(),
    );
    let supply = DriveSupply {
        task_rate,
        transmission_condition: 0.5,
        ..DriveSupply::WHOLE
    };
    let omega = tp.peak_torque_rpm * RPM_TO_RAD;
    let [whole, ..] = engine_torques(&tp, DriveSupply::WHOLE, omega, 1.0);
    let [reduced, ..] = engine_torques(&tp, supply, omega, 1.0);
    assert_eq!(reduced, whole * 0.5);
}

/// The game's driving model has one fuel rack: neither the pedal nor the idle catch may
/// raise net crank torque above the governed full-load curve. Both pricing and work rely on it.
#[test]
fn one_fuel_rack_never_exceeds_the_full_load_curve() {
    for tp in [lab_tp(), tiger_tp()] {
        for fraction in 0..=100 {
            let rpm = (tp.engine.crank_limit_rpm + engine_control().governor_cut_rpm.value)
                * fraction as f32
                / 100.0;
            let omega = rpm * RPM_TO_RAD;
            let actual_rpm = omega / RPM_TO_RAD;
            for command in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let [fuel, catch, drag] = engine_torques(&tp, DriveSupply::WHOLE, omega, command);
                assert!(
                    fuel + catch - drag <= tp.torque_at(actual_rpm) + 1e-3,
                    "rpm={rpm} command={command}: net {} exceeds curve {}",
                    fuel + catch - drag,
                    tp.torque_at(actual_rpm)
                );
            }
        }
    }
}

/// A governor-cut engine must transmit construction drag even under a held pedal.
#[test]
fn the_fuel_cut_includes_drag_reconstruction() {
    let tp = lab_tp();
    let omega = (tp.engine.governed_rpm + engine_control().governor_cut_rpm.value) * RPM_TO_RAD;
    for command in [0.0, 0.5, 1.0] {
        let [fuel, catch, drag] = engine_torques(&tp, DriveSupply::WHOLE, omega, command);
        assert_eq!(
            fuel + catch,
            0.0,
            "the governor cuts every combustion source"
        );
        assert!(drag > 0.0);
    }
}

/// Each reverse gear requires its own steering row before the server ticks.
#[test]
fn reverse_ladder_without_steering_radii_is_refused() {
    let mut authoring = tiger_authoring();
    authoring.reverse_speeds_kmh = &[2.8, 4.3, 6.2, 9.2, 14.1, 20.9, 30.5, 45.4, 60.0];
    let error = TransmissionParams::from_authoring(&authoring)
        .unwrap_err()
        .to_string();
    assert!(error.contains("steering.reverse_radii"), "{error}");
}

/// The game's driving and damage model: a shift's interruption must expire even after
/// control authority leaves. The moving drivetrain must resume transmitting motoring drag.
#[test]
fn empty_station_shift_window_expires_into_motoring_drag() {
    let (fp, tp) = (lab_fp(), lab_tp());
    // Fixture: descending at 5 m/s on a 10-degree grade, authority lost mid-window.
    let speed = 5.0;
    let k = tp.gears_fwd[3] / tp.sprocket_radius;
    let grade = fp.grip.side_weight * 10.0_f32.to_radians().sin();
    let mut st = TransmissionState {
        gear: 4,
        park: false,
        clutch: ClutchState::In,
        shift_ticks: 3,
        omega_e: speed * k,
        ..fresh(&tp)
    };
    let mut inp = input(0.0, 0.0, [speed; 2], [-grade; 2]);
    let first = step(&fp, &tp, &mut st, &inp);
    assert!(first.shifting);
    inp.supply.control_authority = false;

    inp.supply.task_rate = 0.0;
    while st.shift_ticks > 0 {
        assert!(step(&fp, &tp, &mut st, &inp).shifting);
        assert_eq!(st.clutch, ClutchState::Out);
    }
    let report = step(&fp, &tp, &mut st, &inp);
    assert!(!report.shifting);
    assert_eq!(st.clutch, ClutchState::In);
    assert!(report.forces.into_iter().all(|force| force < 0.0));
}

/// The game's driving and damage model: the idle catch is operator work, subject to the
/// Drive task rate. A whole engine alone cannot supply it to propulsion or a neutral pivot.
#[test]
fn idle_catch_requires_working_controls() {
    for tp in [lab_tp(), tiger_tp()] {
        let omega = (tp.engine.idle_rpm - engine_control().idle_guard_band_rpm.value) * RPM_TO_RAD;
        // Fixture: absent authority, absent task throughput, and their independent corners.
        for (control_authority, task_rate) in [(false, 0.0), (false, 1.0), (true, 0.0)] {
            let supply = DriveSupply {
                control_authority,
                task_rate,
                ..DriveSupply::WHOLE
            };
            let mut inp = input(0.0, 0.0, [0.0; 2], [0.0; 2]);
            inp.supply = supply;

            let controls = driver(&construction(&lab_fp()), &tp, &mut fresh(&tp), &inp);
            let [fuel, catch, drag] = engine_torques(&tp, controls.input.supply, omega, 0.0);
            assert_eq!(
                catch, 0.0,
                "catch requires authority and throughput: {supply:?}"
            );
            assert_eq!(fuel, 0.0);
            assert!(drag > 0.0);
        }
    }
}

/// The game's driving and damage model: losing the Driver must remove the catch from a
/// moving, engaged clutch. A staffed zero-rate pivot may spend crank inertia but cannot refill it.
#[test]
fn zero_rate_catch_cannot_sustain_lug_or_pivot() {
    for tp in [lab_tp(), tiger_tp()] {
        let fp = lab_fp();
        // Fixture: a whole drivetrain, zero task rate; a slow engaged coast or pivot.
        for (authority, throttle, steer, speeds) in
            [(true, 0.0, 1.0, [0.0; 2]), (false, 0.0, 0.0, [0.1; 2])]
        {
            let mut st = TransmissionState {
                clutch: ClutchState::In,
                ..fresh(&tp)
            };
            let mut inp = input(throttle, steer, speeds, [0.0; 2]);
            inp.supply.control_authority = authority;

            inp.supply.task_rate = 0.0;
            let mut last = TransmissionReport::default();
            for _ in 0..64 * 4 {
                last = step(&fp, &tp, &mut st, &inp);
            }
            if authority {
                assert_eq!(st.omega_e, 0.0, "a zero-rate pivot cannot sustain idle");
                assert_eq!(last.forces, [0.0; 2], "stored crank energy is finite");
            } else {
                assert_eq!(st.clutch, ClutchState::In);
                assert!(
                    last.forces.into_iter().all(|force| force < 0.0),
                    "a slow coast must receive drag, not catch propulsion: {:?}",
                    last.forces
                );
            }
        }
    }
}

/// The game's driving and damage model: the neutral lever and park lever persist without
/// authority. The clutch springs engaged; neither creates idle fuel. Park holds inside capacity.
#[test]
fn empty_station_at_rest_springs_engaged_and_retains_park() {
    for tp in [lab_tp(), tiger_tp()] {
        let fp = lab_fp();
        let mut st = TransmissionState {
            park: true,
            ..fresh(&tp)
        };
        let mut inp = input(0.0, 0.0, [0.0; 2], [1000.0; 2]); // Fixture grade inside brake capacity.
        inp.supply.control_authority = false;

        inp.supply.task_rate = 0.0;
        for _ in 0..64 * 4 {
            let report = step(&fp, &tp, &mut st, &inp);
            assert_eq!(st.clutch, ClutchState::In);
            assert!(st.park);
            assert_eq!(report.next_speeds, [0.0; 2]);
        }
        assert_eq!(
            st.omega_e, 0.0,
            "without fuel, construction drag stops the crank"
        );
    }
}

/// A moving, already coupled drivetrain is a lug, not a launch. The Driver must retain
/// drive through the ordinary idle guard rather than pay another launch-spool interruption.
#[test]
fn moving_lug_does_not_restart_launch_slip() {
    for tp in [lab_tp(), tiger_tp()] {
        let fp = lab_fp();
        // Fixture: a tall gear below idle on a 20-degree grade, engaged, before a gear correction.
        let gear = (tp.gears_fwd.len() - 2) as u8;
        let k = tp.gears_fwd[(gear - 1) as usize] / tp.sprocket_radius;
        let speed = 0.8 * tp.engine.idle_rpm * RPM_TO_RAD / k;
        let grade = fp.grip.side_weight * 20.0_f32.to_radians().sin();
        let mut st = TransmissionState {
            gear,
            clutch: ClutchState::In,
            ..fresh(&tp)
        };
        let report = step(&fp, &tp, &mut st, &input(1.0, 0.0, [speed; 2], [grade; 2]));
        assert!(
            !report.shifting,
            "fixture must exercise coupling outside a shift"
        );
        assert_eq!(
            st.clutch,
            ClutchState::In,
            "a moving lug must not re-enter launch slip"
        );
        assert!(
            report.forces.into_iter().all(|force| force > 0.0),
            "the lug must retain drive"
        );
    }
}

/// The idle guard relies on a saturated catch at its floor. Invalid control data must be
/// refused before any drivetrain can use the saturation assumption.
#[test]
#[should_panic(expected = "idle_guard_band_rpm")]
fn idle_guard_refuses_a_catch_wider_than_its_band() {
    let mut control: EngineControl = crate::provenance::from_shipped_str(
        ENGINE_CONTROL_PATH,
        include_str!("../../../assets/driving/engine.ron"),
    )
    .unwrap();
    // Fixture: individually positive widths that violate saturation at the guard.
    control.idle_catch_droop_rpm.value = 150.0;
    control.idle_guard_band_rpm.value = 100.0;
    control.validate();
}

/// The pedal blend must retain each requested net-curve share, including its gradual drag
/// compensation. Driver modulation relies on this; a squared-command mutant preserves endpoints.
#[test]
fn part_pedal_preserves_the_net_curve_blend() {
    for tp in [lab_tp(), tiger_tp()] {
        for rpm in [
            tp.peak_torque_rpm,
            (tp.engine.idle_rpm + tp.engine.governed_rpm) * 0.5,
        ] {
            for command in [0.25, 0.5, 0.75] {
                // Fixture part-pedal samples.
                let omega = rpm * RPM_TO_RAD;
                let [fuel, catch, drag] = engine_torques(&tp, DriveSupply::WHOLE, omega, command);
                let expected = command * tp.torque_at(rpm) - {
                    let t =
                        (command / engine_control().drag_throttle_release.value).clamp(0.0, 1.0);
                    1.0 - t * t * (3.0 - 2.0 * t)
                } * engine_drag(&tp, omega);
                assert!(
                    (fuel + catch - drag - expected).abs() < 1e-3,
                    "command {command}, rpm {rpm}: net {} vs {expected}",
                    fuel + catch - drag
                );
            }
        }
    }
}

/// Fixture Driver reading for Driver-policy tests: a hull riding its belts at `speed` and
/// meeting `load` along its heading (forward positive), independent of dynamic belt reactions,
/// so acceleration cannot manufacture a gear change, on ground that grips beyond any gear's pull.
fn driver_input(speed: f32, load: f32, command: f32) -> TransmissionInput {
    let mut inp = input(command, 0.0, [speed; 2], [0.0; 2]);
    inp.sensed = Some(Sensed {
        speed,
        resistance: load,
        push_difference: 0.0,
        drive: load,
        drive_gain: 0.0,
        grip: Some(f32::INFINITY),
        stall: None,
        failed: None,
        stalled: false,
        observed: true,
    });
    inp
}

/// Fixture: `inp`'s hull held at rest under `gear`'s forward pull of the resistance it feels,
/// which is what the ledger reads of a pull that does not move the hull.
fn stalled(mut inp: TransmissionInput, gear: u8) -> TransmissionInput {
    let sensed = inp.sensed.as_mut().expect("a Driver-policy reading");
    sensed.stall = Some(super::super::sense::Stall {
        dir: 1.0,
        push: sensed.resistance,
        gear,
    });
    sensed.stalled = true;
    inp
}

/// The fixture reading a Driver-policy input carries.
fn felt(inp: &TransmissionInput) -> Sensed {
    inp.sensed
        .expect("a Driver-policy fixture carries a reading")
}

/// Fixture oracle: enumerate the declared straight-launch domain using the separately pinned
/// crank/cap force price against the felt resistance. This never consults the selector.
fn affordable_launch(tp: &TransmissionParams, inp: &TransmissionInput, command: f32) -> Option<u8> {
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
            ) - felt(inp).resistance
                > force_margin(tp)
        })
}

/// Under a constant affordable load, the Driver must settle in top gear; players must
/// not hunt between gears after the acceleration transient in each class Fixture.
#[test]
fn driver_full_pedal_walks_to_the_top_on_firm_ground() {
    for (fp, tp) in class_drivetrains() {
        let load = 0.02 * fp.grip.side_weight * 2.0; // Fixture: light supported load.
        let mut state = TransmissionState::from_spec(&tp);
        let mut speeds = [0.0; 2];
        let mut changes = 0;
        let mut last = state.gear;
        for tick in 0..64 * 120 {
            // Fixture: include acceleration, then a settled final second.
            let mut inp = driver_input(speeds[0], load, 1.0);
            inp.speeds = speeds;
            inp.reactions = [load * 0.5; 2];
            speeds = step(&fp, &tp, &mut state, &inp).next_speeds;
            if tick >= 64 * 119 {
                changes += usize::from(state.gear != last);
            }
            last = state.gear;
        }
        println!(
            "MEASURED class settling gears={} final={} speed={} final_second_changes={changes}",
            tp.gears_fwd.len(),
            state.gear,
            speeds[0]
        );
        assert_eq!(usize::from(state.gear), tp.gears_fwd.len());
        assert_eq!(changes, 0, "steady load must settle without gear hunting");
        assert!(
            speeds
                .into_iter()
                .all(|v| v.is_finite() && v > PARK_ENGAGE_SPEED)
        );
    }
}

/// Pricing and delivery share the governed net curve, launch rpm and clutch cap. Fixture
/// alterations independently expose a missing cap, launch-speed substitution or governor.
#[test]
fn driver_force_price_uses_the_current_regime_and_the_clutch_cap() {
    for (_, mut tp) in class_drivetrains() {
        let ratio = tp.gears_fwd[0];
        for command in [0.25, 0.5, 1.0] {
            let omega = (tp.launch_omega(command) / RPM_TO_RAD) * RPM_TO_RAD;
            let [fuel, catch, drag] = engine_torques(&tp, DriveSupply::WHOLE, omega, command);
            for cap in [100.0, 1000.0, 3000.0] {
                tp.clutch_capacity = cap;
                let expected = (fuel + catch - drag).min(cap) * ratio / tp.sprocket_radius;
                let actual = price_driver_force(
                    &tp,
                    DriveSupply::WHOLE,
                    0.0,
                    ratio,
                    command,
                    ClutchState::Slip,
                );
                // Resolution: four f32 rounding steps cover torque-first versus force-first
                // cap conversion; a missing cap or different operating point exceeds this.
                assert!(
                    (actual - expected).abs() <= 4.0 * f32::EPSILON * expected.abs().max(1.0),
                    "price {actual} must match crank/cap force {expected}"
                );
            }
        }
        let speed = (tp.engine.governed_rpm + engine_control().governor_cut_rpm.value)
            * RPM_TO_RAD
            * tp.sprocket_radius
            / ratio;
        assert!(
            price_driver_force(&tp, DriveSupply::WHOLE, speed, ratio, 1.0, ClutchState::In) <= 0.0
        );
        // Fixture: a 1 N m clutch exposes a missing engine-braking cap.
        tp.clutch_capacity = 1.0;
        assert_eq!(
            price_driver_force(&tp, DriveSupply::WHOLE, speed, ratio, 0.0, ClutchState::In),
            -ratio / tp.sprocket_radius
        );
    }
}

/// The declared skip limit and the detent prohibition constrain the same choice path: a stall
/// steps to the lowest gear in reach, never past it. Fixture: a stalled third gear meeting a
/// resistance only first gear's launch can hold, then a straight fourth gear past its governor
/// where fifth has more to spare.
#[test]
fn driver_respects_the_shift_step_and_turn_contracts() {
    let (fp, mut tp) = (lab_fp(), lab_tp());
    let launch = |tp: &TransmissionParams, gear: usize| {
        price_driver_force(
            tp,
            DriveSupply::WHOLE,
            0.0,
            tp.gears_fwd[gear - 1],
            1.0,
            ClutchState::Slip,
        )
    };
    let resistance = (launch(&tp, 1) + launch(&tp, 2)) * 0.5;
    for (max_step, landed) in [(1, 2), (2, 1)] {
        tp.max_step = max_step;
        let mut state = TransmissionState {
            gear: 3,
            clutch: ClutchState::Slip,
            ..fresh(&tp)
        };
        let inp = stalled(driver_input(0.0, resistance, 1.0), 3);
        choose_gear(&mut state, &tp, &construction(&fp), &inp, 1.0, felt(&inp));
        assert_eq!(
            state.gear, landed,
            "a step down reaches no further than max_step {max_step}"
        );
    }
    let governed = tp.engine.governed_rpm + 0.5 * engine_control().governor_cut_rpm.value;
    let speed = governed * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[3];
    for detent in [0, 1, 2] {
        let mut state = TransmissionState {
            gear: 4,
            clutch: ClutchState::In,
            steer_step: detent,
            omega_e: (tp.engine.governed_rpm - 50.0) * RPM_TO_RAD, // Fixture: fuel beats drag.
            ..fresh(&tp)
        };
        let inp = driver_input(speed, 1_000.0, 1.0);
        choose_gear(&mut state, &tp, &construction(&fp), &inp, 1.0, felt(&inp));
        assert_eq!(
            state.gear,
            if detent == 0 { 5 } else { 4 },
            "a detent turn's radius belongs to its gear"
        );
    }
}

/// The top of any admitted ladder chooses no gear above it: the upshift clause relies on a
/// higher gear existing before it forms the range above the engaged one. Fixture: a ladder of the
/// most gears the u8 index admits, cruising straight in its top gear on ample grip.
#[test]
fn the_top_of_the_longest_admitted_ladder_chooses_nothing_above_it() {
    let speeds: Vec<f32> = (1..=u8::MAX).map(|gear| f32::from(gear) * 0.5).collect();
    let radii = vec![(10.0, 30.0); speeds.len()];
    let tp = TransmissionParams::from_authoring(&TransmissionAuthoring {
        forward_speeds_kmh: &speeds,
        steer_radii_m: &radii,
        ..tiger_authoring()
    })
    .expect("a ladder of u8::MAX gears is admitted");
    let top = u8::MAX;
    let speed = 0.5 * tp.governed_speed(false);
    let mut state = TransmissionState {
        gear: top,
        clutch: ClutchState::In,
        omega_e: (tp.engine.governed_rpm - 50.0) * RPM_TO_RAD, // Fixture: fuel beats drag.
        ..fresh(&tp)
    };
    let inp = driver_input(speed, 1_000.0, 1.0);
    choose_gear(
        &mut state,
        &tp,
        &construction(&lab_fp()),
        &inp,
        1.0,
        felt(&inp),
    );
    assert_eq!(state.gear, top);
}

/// A gear left downward is retried once the hull meets less than it sustained or a retry would
/// turn its crank faster than it failed at, and not before: the memory must hold a retry back no
/// further than the operating point it observed, and no less. Fixture: each class drivetrain
/// cruising in first where second qualifies, with second remembered on each side of both bounds.
#[test]
fn a_left_gear_waits_for_less_resistance_or_a_faster_crank() {
    for (fp, tp) in class_drivetrains() {
        let construction = construction(&fp);
        let ratio = tp.gears_fwd[0];
        let cruising = |rpm: f32| TransmissionState {
            gear: 1,
            clutch: ClutchState::In,
            omega_e: rpm * RPM_TO_RAD,
            ..fresh(&tp)
        };
        let span = tp.engine.governed_rpm - tp.engine.idle_rpm;
        let (rpm, mut inp) = (1..24)
            .flat_map(|step| (1..=20).map(move |share| (step, share)))
            .find_map(|(step, share)| {
                let rpm = tp.engine.idle_rpm + span * step as f32 / 24.0;
                let speed = rpm * RPM_TO_RAD * tp.sprocket_radius / ratio;
                let inp = driver_input(speed, launch_force(&tp, 1) * share as f32 / 40.0, 1.0);
                let mut st = cruising(rpm);
                choose_gear(&mut st, &tp, &construction, &inp, 1.0, felt(&inp));
                (st.gear == 2).then_some((rpm, inp))
            })
            .expect("a cruise in first where second qualifies");
        let resistance = felt(&inp).resistance;
        for (push, crank, retried) in [
            (0.5 * resistance, f32::INFINITY, false),
            (0.5 * resistance, 0.0, true),
            (2.0 * resistance, f32::INFINITY, true),
        ] {
            inp.sensed.as_mut().expect("a Driver-policy reading").failed =
                Some(super::super::sense::Failed {
                    gear: (false, 2),
                    push,
                    crank,
                    command: 1.0,
                    steer_step: 0,
                });
            let mut st = cruising(rpm);
            choose_gear(&mut st, &tp, &construction, &inp, 1.0, felt(&inp));
            assert_eq!(
                st.gear == 2,
                retried,
                "second sustained {push} N against {resistance} N, failed at {crank} rad/s"
            );
        }
    }
}

/// Fixture launch price of forward `gear` at full pedal: what a slipping clutch delivers.
fn launch_force(tp: &TransmissionParams, gear: u8) -> f32 {
    price_driver_force(
        tp,
        DriveSupply::WHOLE,
        0.0,
        tp.gears_fwd[usize::from(gear - 1)],
        1.0,
        ClutchState::Slip,
    )
}

/// No gear is priced above the grip its belts can carry: belts sliding along their travel at
/// their grip limit, carrying the whole resistance, change no gear, where the same reading on
/// ample grip changes gears. A Driver that shifted on an ungranted pull would cut the drive for
/// nothing. Fixture: each class drivetrain moving in each forward gear with the clutch in, the
/// crank from idle to governed and resistance up to first gear's launch pull, every patch sliding
/// along the belt so that its grip budget is the resistance.
#[test]
fn belts_at_their_grip_limit_change_no_gear() {
    for (fp, tp) in class_drivetrains() {
        let construction = construction(&fp);
        let mut changed = 0;
        for gear in 1..=tp.gears_fwd.len() as u8 {
            let ratio = tp.gears_fwd[usize::from(gear - 1)];
            for step in 0..=24 {
                let span = tp.engine.governed_rpm - tp.engine.idle_rpm;
                let rpm = tp.engine.idle_rpm + span * step as f32 / 24.0;
                let speed = rpm * RPM_TO_RAD * tp.sprocket_radius / ratio;
                for share in 0..=20 {
                    let resistance = launch_force(&tp, 1) * share as f32 / 20.0;
                    let mut inp = driver_input(speed, resistance, 1.0);
                    let engaged = TransmissionState {
                        gear,
                        clutch: ClutchState::In,
                        omega_e: rpm * RPM_TO_RAD,
                        ..fresh(&tp)
                    };
                    let mut st = engaged;
                    choose_gear(&mut st, &tp, &construction, &inp, 1.0, felt(&inp));
                    changed += usize::from(st.gear != gear);
                    let sliding = inp.sensed.as_mut().expect("a Driver-policy reading");
                    sliding.grip = Some(resistance);
                    let mut st = engaged;
                    choose_gear(&mut st, &tp, &construction, &inp, 1.0, felt(&inp));
                    assert_eq!(
                        st.gear, gear,
                        "gear {gear} at {rpm} rpm against {resistance} N, sliding"
                    );
                }
            }
        }
        assert!(changed > 0, "the fixture's ample grip changes gears");
    }
}

/// A Driver that has felt nothing has chosen no gear, so it engages none from rest: the launch's
/// free choice relies on the clutch still being open once the first reading arrives.
#[test]
fn nothing_felt_engages_nothing_from_rest() {
    for (fp, tp) in class_drivetrains() {
        let mut state = fresh(&tp);
        let report = step(&fp, &tp, &mut state, &input(1.0, 0.0, [0.0; 2], [0.0; 2]));
        assert_eq!(
            state.clutch,
            ClutchState::Out,
            "an unfelt pedal engages nothing"
        );
        assert_eq!(report.forces, [0.0; 2]);
        step(
            &fp,
            &tp,
            &mut state,
            &felt_at_rest(input(1.0, 0.0, [0.0; 2], [0.0; 2])),
        );
        assert_eq!(
            state.clutch,
            ClutchState::Slip,
            "the first reading launches"
        );
    }
}

/// The free choice at hull rest is the highest launch gear that holds what the Driver feels,
/// and costs no cut: the launch relies on it. Fixture: every starting gear against no
/// resistance, a resistance between the launch prices, and one no launch gear holds.
#[test]
fn the_free_choice_at_rest_launches_in_the_highest_launch_gear_that_holds() {
    for (fp, tp) in class_drivetrains() {
        let top = tp.gears_fwd.len() as u8;
        let lowest = tp.launch_gears.min(top);
        for resistance in [
            0.0,
            (launch_force(&tp, 1) + launch_force(&tp, lowest)) * 0.5,
            launch_force(&tp, 1) * 1.5,
        ] {
            let inp = driver_input(0.0, resistance, 1.0);
            let expected = affordable_launch(&tp, &inp, 1.0).unwrap_or(1);
            for start in 1..=top {
                let mut state = TransmissionState {
                    gear: start,
                    ..fresh(&tp)
                };
                choose_gear(&mut state, &tp, &construction(&fp), &inp, 1.0, felt(&inp));
                assert_eq!(state.gear, expected, "R={resistance} from gear {start}");
                assert_eq!(state.shift_ticks, 0, "the choice at rest is free");
            }
        }
    }
}

/// A hull at rest with its clutch engaged never upshifts: every higher gear launches with less
/// force. A stalled push relies on it to step down, never up. Fixture: each class and gear,
/// slipping or engaged, against no resistance, half and all of first gear's launch.
#[test]
fn no_upshift_at_hull_rest() {
    for (fp, tp) in class_drivetrains() {
        for start in 1..=tp.gears_fwd.len() as u8 {
            for clutch in [ClutchState::Slip, ClutchState::In] {
                for share in [0.0, 0.5, 1.0] {
                    let inp = driver_input(0.0, share * launch_force(&tp, 1), 1.0);
                    let mut state = TransmissionState {
                        gear: start,
                        clutch,
                        ..fresh(&tp)
                    };
                    choose_gear(&mut state, &tp, &construction(&fp), &inp, 1.0, felt(&inp));
                    assert!(
                        state.gear <= start,
                        "{clutch:?} gear {start} at rest rose to {}",
                        state.gear
                    );
                }
            }
        }
    }
}

/// A full-pedal push held at rest by an obstacle steps down to first gear and never up: the
/// stalled hull's push is what it feels, and every lower gear launches with more. Fixture: hull
/// and belts held at rest, the obstacle returning the push the Driver felt a tick earlier.
#[test]
fn a_stalled_full_clutch_push_steps_down() {
    for (fp, tp) in class_drivetrains() {
        let construction = construction(&fp);
        let mass = construction.weight_n() / super::super::derive::G;
        let mut sense = super::super::sense::DriverSense::for_cut(tp.shift_ticks);
        let mut state = TransmissionState {
            gear: tp.launch_gears.min(tp.gears_fwd.len() as u8),
            clutch: ClutchState::Slip,
            ..fresh(&tp)
        };
        let mut pushed = [0.0_f32; 2];
        let mut gears = vec![state.gear];
        // Fixture: ten seconds for every step down.
        for _ in 0..64 * 10 {
            let mut inp = input(1.0, 0.0, [0.0; 2], pushed);
            let forward = bevy::math::Vec3::NEG_Z;
            let hard = pushed.map(|push| {
                Some(super::super::stop_contacts::HardReaction {
                    belt: push,
                    friction: forward * push,
                })
            });
            inp.sensed = sense.tick(
                bevy::math::Vec3::ZERO,
                forward,
                0.0,
                mass,
                super::super::sense::Contacts {
                    soft: [bevy::math::Vec3::ZERO; 2],
                    grip: 0.0,
                    hard,
                },
                inp.dt,
            );
            let report = step(&fp, &tp, &mut state, &inp);
            sense.drove(&report);
            pushed = report.forces;
            if gears.last() != Some(&state.gear) {
                assert!(
                    state.gear < *gears.last().unwrap(),
                    "a stalled push rose: {gears:?}"
                );
                gears.push(state.gear);
            }
        }
        println!("MEASURED stalled push gears={gears:?}");
        assert_eq!(
            state.gear, 1,
            "a stalled push ends in first gear: {gears:?}"
        );
    }
}

/// Belts spinning free under a hull that does not move change no gear: grip they do not have
/// carries no gear's pull, so none offers more than the gear engaged. Fixture: a hull at rest
/// feeling nothing while its belts meet no ground.
#[test]
fn free_spinning_belts_change_no_gear() {
    for (fp, tp) in class_drivetrains() {
        let launch = tp.launch_gears.min(tp.gears_fwd.len() as u8);
        let mut state = TransmissionState {
            gear: launch,
            clutch: ClutchState::Slip,
            ..fresh(&tp)
        };
        let mut speeds = [0.0; 2];
        for _ in 0..64 * 10 {
            let inp = felt_free_at_rest(input(1.0, 0.0, speeds, [0.0; 2]));
            let report = step(&fp, &tp, &mut state, &inp);
            assert!(
                !report.shifting && state.gear == launch,
                "belts at {speeds:?} moved the gear"
            );
            speeds = report.next_speeds;
        }
        assert!(speeds[0] > 1.0, "the fixture belts spin: {speeds:?}");
    }
}

/// The clutch guard bounds the crank independently of the gear policy; a top-gear overrun
/// must retain the construction's motoring drag. Both fixture curves exercise the same law.
#[test]
fn overrun_slip_guard_bounds_crank_at_top_gear() {
    let fp = lab_fp();
    for tp in [lab_tp(), tiger_tp()] {
        let top = tp.gears_rev.len() as u8;
        let ratio = tp.gears_rev[usize::from(top - 1)];
        let guard = (tp.engine.crank_limit_rpm + overrev_margin_rpm()) * RPM_TO_RAD;
        let speed = -2.0 * guard * tp.sprocket_radius / ratio;
        let mut state = fresh(&tp);
        state.gear = top;
        state.reverse = true;
        let mut report = TransmissionReport::default();
        let mut inp = input(0.0, 0.0, [speed; 2], [40_000.0; 2]);
        inp.supply.control_authority = false;

        for _ in 0..64 {
            report = step(&fp, &tp, &mut state, &inp);
            assert!(state.omega_e <= guard + 0.001);
            assert_eq!(state.gear, top);
        }
        let expected =
            engine_drag(&tp, guard).min(tp.clutch_capacity) * ratio / tp.sprocket_radius / 2.0;
        for force in report.forces {
            assert!((force - expected).abs() < expected * 0.001);
        }
    }
}

/// A paid shift disconnects propulsion for exactly the construction's interruption; no
/// extra confirmation or dwell may lengthen it; the clutch re-engages afterward.
#[test]
fn paid_shift_disconnects_for_exactly_its_declared_window() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut state = fresh(&tp);
    state.shift_ticks = tp.shift_ticks.get();
    let speed = tp.engine.idle_rpm * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[0];
    for _ in 0..tp.shift_ticks.get() {
        let report = step(&fp, &tp, &mut state, &input(1.0, 0.0, [speed; 2], [0.0; 2]));
        assert!(report.shifting);
        assert_eq!(report.forces, [0.0; 2]);
    }
    let report = step(&fp, &tp, &mut state, &input(1.0, 0.0, [speed; 2], [0.0; 2]));
    assert!(!report.shifting);
    assert_ne!(state.clutch, ClutchState::Out);
}

/// The gear deadband must cover actual same-instant arithmetic on lighter and heavier rigs
/// even when their engine is unchanged. A multiplication-only test cannot detect an undersized M.
#[test]
#[cfg(feature = "bitprobe")]
fn landing_resolution_bounds_independent_weight_rigs() {
    for mass in [20_000.0, 60_000.0] {
        // Fixture masses, same engine and gearbox at both weights.
        for tp in [lab_tp(), tiger_tp()] {
            let mut fp = lab_fp();
            fp.grip.side_weight = mass * super::super::derive::G / 2.0;
            let mut residuals = Vec::new();
            for (index, &ratio) in tp.gears_fwd.iter().enumerate() {
                for command in [0.25, 0.5, 1.0] {
                    // Fixture pedal samples.
                    for rpm in [
                        tp.engine.idle_rpm,
                        tp.peak_torque_rpm,
                        tp.engine.governed_rpm,
                    ] {
                        let speed = rpm * RPM_TO_RAD * tp.sprocket_radius / ratio;
                        let mut state = TransmissionState {
                            gear: (index + 1) as u8,
                            clutch: ClutchState::In,
                            omega_e: rpm * RPM_TO_RAD,
                            ..fresh(&tp)
                        };
                        // No support observation isolates the mechanical solve from a gear decision.
                        let inp = input(
                            command,
                            0.0,
                            [speed; 2],
                            [mass * super::super::derive::G * 0.04 / 2.0; 2],
                        );
                        let probe = step(&fp, &tp, &mut state, &inp).bitprobe;
                        let acceleration = (probe.omega_integrated - probe.omega_pre) / probe.dt;
                        let priced = (probe.tau_free - tp.engine_inertia * acceleration) * probe.k;
                        residuals.push(
                            priced - probe.direction * probe.f_c_pre_scale * probe.power_scale,
                        );
                    }
                }
            }
            let lo = residuals.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = residuals.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            println!(
                "MEASURED independent-weight residual mass_kg={mass} gears={} min_n={lo} max_n={hi} margin_n={}",
                tp.gears_fwd.len(),
                force_margin(&tp)
            );
            let crank_margin = force_margin(&tp);
            assert!(residuals.iter().all(|value| value.is_finite()));
            assert!(
                (hi - lo).max(lo.abs()).max(hi.abs()) < crank_margin,
                "same-instant residual span must fit the crank arithmetic bound: {lo}..{hi} vs {crank_margin}"
            );
        }
    }
}

/// The requested pedal chooses gears while the physical pedal is still travelling.
/// The driver adapter must not replace intent with its shaped actuator value.
#[test]
fn gear_choice_reads_requested_pedal_before_its_ramp() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let gear = (tp.gears_fwd.len() - 1) as u8;
    let speed = tp.engine.governed_rpm * RPM_TO_RAD * tp.sprocket_radius
        / tp.gears_fwd[usize::from(gear - 1)];
    for (requested, shaped, shifts) in [(1.0, 0.0, true), (1.0, 1.0, true), (0.0, 1.0, false)] {
        let mut state = TransmissionState {
            gear,
            clutch: ClutchState::In,
            omega_e: tp.peak_torque_rpm * RPM_TO_RAD,
            ..fresh(&tp)
        };
        let mut inp = driver_input(speed, 1000.0, requested);
        inp.throttle = shaped;
        let report = step(&fp, &tp, &mut state, &inp);
        assert_eq!(report.shifting, shifts);
    }
}

/// A declared capable launch must actually gain speed, for both construction fixtures;
/// pricing alone cannot prove the clutch, brake latch and selector agree.
#[test]
fn capable_launches_gain_speed_against_a_supported_grade() {
    for (fp, tp) in class_drivetrains() {
        let load = 0.1 * fp.grip.side_weight * 2.0; // Fixture incline load, within grip.
        let mut state = TransmissionState {
            park: true,
            ..fresh(&tp)
        };
        let mut speeds = [0.0; 2];
        let initial = driver_input(0.0, load, 1.0);
        assert!(affordable_launch(&tp, &initial, 1.0).is_some());
        for _ in 0..64 * 10 {
            // Fixture: ten simulated seconds to include crank spool.
            let mut inp = driver_input((speeds[0] + speeds[1]) * 0.5, load, 1.0);
            inp.speeds = speeds;
            inp.reactions = [load * 0.5; 2];
            speeds = step(&fp, &tp, &mut state, &inp).next_speeds;
        }
        assert!(
            speeds.into_iter().all(|speed| speed > PARK_ENGAGE_SPEED),
            "a capable supported launch must gain speed: {speeds:?}"
        );
    }
}

/// The player's opposite-key hold must brake completely before a direction cut. Releasing
/// during the authored pause leaves the park lever on, with no reverse motion.
#[test]
fn reversal_release_during_hold_parks_without_a_direction_cut() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut state = fresh(&tp);
    let mut speeds = [5.0; 2]; // Fixture: forward cruise.
    for _ in 0..64 * 10 {
        let was_rest = speeds.iter().all(|v: &f32| v.abs() < PARK_ENGAGE_SPEED);
        let report = step(&fp, &tp, &mut state, &input(-1.0, 0.0, speeds, [0.0; 2]));
        assert!(
            !state.reverse,
            "direction cannot change before the rest hold"
        );
        speeds = report.next_speeds;
        if was_rest {
            break;
        }
    }
    assert!(speeds.iter().all(|v| v.abs() < PARK_ENGAGE_SPEED));
    for _ in 0..64 {
        speeds = step(&fp, &tp, &mut state, &input(0.0, 0.0, speeds, [0.0; 2])).next_speeds;
        assert!(!state.reverse);
        assert!(state.park);
        assert!(speeds.iter().all(|v| v.abs() < 1e-5));
    }
}

/// Part-pedal intent must launch and sustain forward travel; a full-pedal launch price cannot
/// select a gear that a partial pedal cannot pull. Fixture: constant level rolling load.
#[test]
fn part_pedal_launches_and_sustains_ordered_speed() {
    for (fp, tp) in class_drivetrains() {
        let mut capable_notches = 0;
        let weight = fp.grip.side_weight * 2.0;
        for degrees in [0.0_f32, 10.0, 20.0] {
            // Fixture: level and supported grades.
            let grade = weight * degrees.to_radians().sin();
            let rolling = 0.015 * weight * degrees.to_radians().cos(); // Fixture: firm ground.
            let mut last_speed = 0.0;
            for command in [0.25, 0.5, 0.75] {
                let mut state = fresh(&tp);
                let mut speed = 0.0;
                let mut lo = f32::INFINITY;
                let mut hi = f32::NEG_INFINITY;
                let initial = driver_input(0.0, grade + rolling, command);
                let capable = (1..=tp.launch_gears.min(tp.gears_fwd.len() as u8)).any(|gear| {
                    price_driver_force(
                        &tp,
                        initial.supply,
                        0.0,
                        tp.gears_fwd[usize::from(gear - 1)],
                        command,
                        ClutchState::Slip,
                    ) - felt(&initial).resistance
                        > force_margin(&tp)
                });
                capable_notches += usize::from(capable);
                for tick in 0..64 * 60 {
                    let mut inp = driver_input(speed, grade + rolling, command);
                    inp.reactions = [(grade
                        + rolling * (speed / forces::ROLLING_FADE_SPEED).clamp(-1.0, 1.0))
                        * 0.5; 2];
                    speed = step(&fp, &tp, &mut state, &inp).next_speeds[0];
                    assert!(
                        speed >= -PARK_ENGAGE_SPEED,
                        "a supported notch cannot roll backward"
                    );
                    if tick >= 64 * 59 {
                        lo = lo.min(speed);
                        hi = hi.max(speed);
                    }
                }
                println!(
                    "MEASURED notch fixture gears={} grade={degrees} command={command} capable={capable} gear={} speed={speed} spread={}",
                    tp.gears_fwd.len(),
                    state.gear,
                    hi - lo
                );
                if capable {
                    assert!(
                        speed > PARK_ENGAGE_SPEED,
                        "every capable notch must sustain travel"
                    );
                    assert!(speed > last_speed, "more pedal must sustain more speed");
                    // Resolution: final-window ripple below one percent of the declared speed scale.
                    assert!(
                        hi - lo < 0.01 * tp.governed_speed(false),
                        "the notch must settle"
                    );
                    last_speed = speed;
                }
            }
        }
        assert!(
            capable_notches > 0,
            "each class must exercise capable partial-pedal travel"
        );
    }
}

/// A held-intent stop is a fresh launch regardless of the prior clutch regime. The brake law
/// relies on this to release the one-way lever after an arrested climb.
#[test]
fn law_arrested_climb_reenters_launch_slip() {
    for tp in [lab_tp(), tiger_tp()] {
        let mut state = TransmissionState {
            park: false,
            clutch: ClutchState::In,
            ..fresh(&tp)
        };
        let inp = felt_at_rest(input(1.0, 0.0, [0.0; 2], [0.0; 2]));
        driver(&construction(&lab_fp()), &tp, &mut state, &inp);
        assert_eq!(
            state.clutch,
            ClutchState::Slip,
            "arrested intent must not remain an engaged lug"
        );
    }
}

/// Fixture: nominal 57-tonne construction and its derived 4,750 kg reflected belt.
fn grade_fixture_fp() -> FixtureConstruction {
    let mut fp = lab_fp();
    fp.inertia = TIGER_BELT_INERTIA;
    fp.grip.side_weight = 57_000.0 * super::super::derive::G / 2.0;
    fp
}

/// The player must receive the complete stopped pause followed by exactly one direction cut.
/// Fixture: full-speed entry on each construction, with a full opposite-key hold.
#[test]
fn held_reversal_waits_at_rest_and_pays_exactly_one_cut() {
    for (fp, tp) in class_drivetrains() {
        for (reverse, degrees) in [(false, 0.0_f32), (false, 10.0), (true, -10.0)] {
            let dir = if reverse { -1.0 } else { 1.0 };
            let grade = fp.grip.side_weight * 2.0 * degrees.to_radians().sin();
            let mut state = TransmissionState {
                gear: if reverse {
                    tp.gears_rev.len() as u8
                } else {
                    tp.gears_fwd.len() as u8
                },
                reverse,
                ..fresh(&tp)
            };
            let mut speeds = [tp.governed_speed(reverse) * dir; 2];
            let mut held_ticks = 0;
            let mut cuts = 0;
            let mut cut_ticks = 0;
            let mut was_cut = false;
            let hold = (driver_actuation().reversal_hold_s.value * TICK_HZ).ceil() as u8;
            for _ in 0..64 * 30 {
                let at_rest = speeds.iter().all(|v| v.abs() < PARK_ENGAGE_SPEED);
                if at_rest && state.reverse == reverse {
                    held_ticks += 1;
                }
                let mut inp = driver_input(speeds[0], grade, -dir);
                inp.speeds = speeds;
                inp.reactions = [grade * 0.5; 2];
                let report = step(&fp, &tp, &mut state, &inp);
                if state.reverse != reverse && !was_cut && report.shifting {
                    assert!(at_rest, "direction cannot change while rolling");
                    assert_eq!(
                        held_ticks,
                        usize::from(hold) + 1,
                        "all stopped hold ticks precede the swap"
                    );
                }
                cuts += usize::from(report.shifting && !was_cut);
                cut_ticks += usize::from(report.shifting);
                was_cut = report.shifting;
                if state.reverse == reverse {
                    assert!(
                        report
                            .next_speeds
                            .iter()
                            .all(|v| *v * dir >= -PARK_ENGAGE_SPEED)
                    );
                }
                speeds = report.next_speeds;
                if state.reverse != reverse
                    && !report.shifting
                    && speeds.iter().all(|v| *v * dir < -PARK_ENGAGE_SPEED)
                {
                    break;
                }
            }
            println!(
                "MEASURED reversal hold_ticks={} cut_ticks={cut_ticks} cuts={cuts} final_speed={}",
                held_ticks - 1,
                speeds[0]
            );
            assert!(state.reverse != reverse && speeds[0] * dir < -PARK_ENGAGE_SPEED);
            assert_eq!(cuts, 1);
            assert_eq!(cut_ticks, usize::from(tp.shift_ticks.get()));
        }
    }
}

/// Coasting above first-gear idle must not apply the terrain lever. A lever-on-coast
/// mutant distinguishes the supported run; crank drag has its own coupling witness.
#[test]
fn released_motion_coasts_without_brake_force() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let speed = tp.governed_speed(false) * 0.5; // Fixture: moving well above the stop threshold.
    let inp = input(0.0, 0.0, [speed; 2], [1000.0; 2]);
    let mut state = TransmissionState {
        park: false,
        clutch: ClutchState::In,
        gear: 3,
        ..fresh(&tp)
    };
    let controls = driver(&construction(&fp), &tp, &mut state, &inp);
    let mut q = [0.0; 2];
    let belts = BeltTick::new(&construction(&fp), &controls.input);
    apply_brake_stop_forces(&mut q, &tp, &belts, &state, &controls);
    assert_eq!(q, [0.0; 2]);
}

/// The one-way lever must carry the launch shortfall without opposing useful motion, and the
/// launch from an open clutch pays no cut. A supported launch on 20 degrees uses construction
/// inertia and never rolls out of the rest band.
#[test]
fn one_way_lever_launches_on_grade_without_rollback_or_a_launch_cut() {
    for (fp, tp) in class_drivetrains() {
        let load = fp.grip.side_weight * 2.0 * 20.0_f32.to_radians().sin();
        let initial = driver_input(0.0, load, 1.0);
        let launch = affordable_launch(&tp, &initial, 1.0).expect("a supported grade");
        let spare = price_driver_force(
            &tp,
            initial.supply,
            0.0,
            tp.gears_fwd[usize::from(launch - 1)],
            1.0,
            ClutchState::Slip,
        ) - load;
        let mut state = fresh(&tp);
        let mut speed = 0.0;
        let mut cuts = 0;
        let mut last_cut = false;
        let mut minimum = 0.0_f32;
        for tick in 0..64 * 20 {
            let mut inp = driver_input(speed, load, 1.0);
            inp.reactions = [load * 0.5; 2];
            let report = step(&fp, &tp, &mut state, &inp);
            if tick == 0 {
                assert_eq!(state.gear, launch, "the launch takes the gear that holds");
            }
            assert!(
                !report.shifting || speed >= PARK_ENGAGE_SPEED,
                "a launch from rest pays no cut"
            );
            cuts += usize::from(report.shifting && !last_cut);
            last_cut = report.shifting;
            speed = report.next_speeds[0];
            minimum = minimum.min(speed);
            assert!(report.next_speeds.iter().all(|v| *v >= -PARK_ENGAGE_SPEED));
        }
        println!(
            "MEASURED S2 gears={} belt_kg={} launch={launch} gear={} spare_n={spare} load_n={load} cuts={cuts} min_speed={minimum} final_speed={speed}",
            tp.gears_fwd.len(),
            fp.inertia,
            state.gear
        );
        assert!(speed > PARK_ENGAGE_SPEED);
    }
}

/// The reversal pause and cut must hold both directions before launch. Driver control relies
/// on this when gravity pulls in the new request's direction. Fixture: stopped on a 10-degree face.
#[test]
fn reversal_pause_and_cut_hold_a_grade_in_both_directions() {
    for (fp, tp) in [(lab_fp(), lab_tp()), (grade_fixture_fp(), tiger_tp())] {
        for command in [-1.0_f32, 1.0] {
            let load = -command * fp.grip.side_weight * 2.0 * 10.0_f32.to_radians().sin();
            let mut state = TransmissionState {
                reverse: command > 0.0,
                park: true,
                reversal_hold_ticks: (driver_actuation().reversal_hold_s.value * TICK_HZ).ceil()
                    as u8,
                ..fresh(&tp)
            };
            let mut speed = 0.0;
            let mut cuts = 0;
            let mut was_cut = false;
            for _ in 0..128 {
                let mut inp = driver_input(speed, load, command);
                inp.reactions = [load * 0.5; 2];
                let report = step(&fp, &tp, &mut state, &inp);
                cuts += usize::from(report.shifting && !was_cut);
                was_cut = report.shifting;
                speed = report.next_speeds[0];
                if state.reverse != (command < 0.0) || report.shifting {
                    assert!(
                        speed.abs() < 1e-5,
                        "reversal pause/cut drifted at {speed} m/s"
                    );
                } else if speed * command > PARK_ENGAGE_SPEED {
                    break;
                }
            }
            assert_eq!(state.reverse, command < 0.0);
            assert_eq!(cuts, 1);
            assert!(speed * command > PARK_ENGAGE_SPEED);
            assert!(state.gear <= tp.launch_gears);
        }
    }
}

/// Releasing between solver rest and first-gear idle must stop and park. Removing the
/// construction-derived stop clause leaves this moving entry unlatched.
#[test]
fn released_sub_idle_motion_stops_and_parks() {
    for (fp, tp) in [(lab_fp(), lab_tp()), (grade_fixture_fp(), tiger_tp())] {
        let idle_speed = tp.engine.idle_rpm * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[0];
        let speed = (idle_speed + PARK_ENGAGE_SPEED) * 0.5;
        let mut state = TransmissionState {
            park: false,
            ..fresh(&tp)
        };
        let mut speeds = [speed; 2];
        for _ in 0..8 {
            speeds = step(&fp, &tp, &mut state, &input(0.0, 0.0, speeds, [0.0; 2])).next_speeds;
            assert!(state.park);
        }
        assert!(speeds.iter().all(|v| v.abs() < 1e-5));
    }
}

/// A backward engaged crank cannot fire, and its drag must oppose rotation. Removing the
/// signed loss or admitting combustion would let an empty station freewheel through rollback.
#[test]
fn backward_crank_has_only_opposing_motoring_drag() {
    for tp in [lab_tp(), tiger_tp()] {
        let omega = -tp.engine.governed_rpm * RPM_TO_RAD;
        let [fuel, catch, drag] = engine_torques(&tp, DriveSupply::WHOLE, omega, 1.0);
        assert_eq!([fuel, catch], [0.0; 2]);
        assert!(drag < 0.0);
        assert_eq!(drag, -engine_drag(&tp, -omega));
    }
}

/// The Driver must brake at the curve ceiling without buying a shift; without authority,
/// only the independent clutch guard remains. Fixture: overrun below top gear
/// where the ladder permits it, with a live drive request so gear holding exercises C2.
#[test]
fn ceiling_brake_requires_a_driver_and_holds_the_current_gear() {
    for (fp, tp) in class_drivetrains() {
        for reverse in [false, true] {
            let dir = if reverse { -1.0 } else { 1.0 };
            let ladder = if reverse {
                &tp.gears_rev
            } else {
                &tp.gears_fwd
            };
            let gear = ladder.len().saturating_sub(1).max(1) as u8;
            let ratio = ladder[usize::from(gear - 1)];
            let k = ratio / tp.sprocket_radius;
            let ceiling = tp.engine.crank_limit_rpm * RPM_TO_RAD / k;
            let speed = dir * ceiling;
            let mut inp = input(
                dir * 0.25, // Fixture: requested drive remains below drag, so C2 must hold.
                0.0,
                [speed; 2],
                [-dir * (engine_drag(&tp, ceiling * k) * k * 0.5 + tp.brake_capacity_n * 0.25); 2],
            );
            inp.sensed = driver_input(speed, inp.reactions.iter().sum(), 0.25 * dir).sensed;
            let mut state = TransmissionState {
                gear,
                reverse,
                park: false,
                clutch: ClutchState::In,
                omega_e: tp.engine.crank_limit_rpm * RPM_TO_RAD,
                ..fresh(&tp)
            };
            let initial = state;
            let staffed = step(&fp, &tp, &mut state, &inp);
            assert!(!staffed.shifting);
            assert_eq!(state.gear, gear);
            // Resolution: f32 integration at the construction-derived ceiling.
            assert!(staffed.next_speeds[0] * dir <= ceiling + 1e-5);
            inp.supply.control_authority = false;

            let unworked = step(&fp, &tp, &mut { initial }, &inp);
            // Resolution: distinguish braking from f32 integration roundoff.
            assert!(
                unworked.next_speeds[0] * dir > staffed.next_speeds[0] * dir + 1e-5,
                "ceiling brakes require a staffed Driver"
            );
        }
    }
}

/// Open-clutch rest cannot invent rev-match work. The Driver's catch is accounted separately;
/// the window blip requires both authority and an actual paid interruption.
#[cfg(feature = "bitprobe")]
#[test]
fn rev_match_requires_a_window_or_steering_and_authority() {
    let tp = lab_tp();
    for authority in [false, true] {
        for shifting in [false, true] {
            let mut inp = input(1.0, 0.0, [5.0; 2], [0.0; 2]);
            inp.supply.control_authority = authority;

            let controls = driver(&construction(&lab_fp()), &tp, &mut fresh(&tp), &inp);
            let inp = &controls.input;
            let mut probe = TransmissionProbe::default();
            crank_free_torque(
                &tp,
                inp,
                false,
                shifting,
                0.0,
                5.0,
                100.0,
                tp.engine.idle_rpm * RPM_TO_RAD,
                tp.engine.idle_rpm * RPM_TO_RAD,
                &mut probe,
            );
            assert_eq!(probe.tau_induced > 0.0, authority && shifting);
        }
    }
}

/// A sliding reversal retains its direction and refreshes the pause; only rest permits a cut.
/// Fixture: grade force exceeds both static and dynamic brake capacity.
#[test]
fn reversal_beyond_brake_capacity_refuses_the_direction_cut() {
    let (fp, tp) = (grade_fixture_fp(), tiger_tp());
    let load = tp.brake_capacity_n * tp.brake_static_factor * 3.0; // Fixture: beyond static and service capacity.
    let mut state = TransmissionState {
        park: false,
        ..fresh(&tp)
    };
    let mut speed = 2.0;
    for _ in 0..64 * 5 {
        let mut inp = input(-1.0, 0.0, [speed; 2], [load * 0.5; 2]);
        inp.sensed = driver_input(speed, load, -1.0).sensed;
        let report = step(&fp, &tp, &mut state, &inp);
        speed = report.next_speeds[0];
        assert!(!state.reverse && !report.shifting);
    }
    assert!(speed < -PARK_ENGAGE_SPEED);
    assert!(state.reversal_hold_ticks > 0);
}

/// The game's driving and damage model: a staffed Driver retains the park lever when
/// either powertrain HP pool is emptied. Players can stop a coast and hold at rest.
#[test]
fn staffed_zero_rate_stops_and_parks_without_engine_work() {
    for (fp, tp) in [(lab_fp(), lab_tp()), (grade_fixture_fp(), tiger_tp())] {
        for engine_empty in [false, true] {
            let supply = DriveSupply {
                task_rate: 0.0,
                engine_condition: if engine_empty { 0.0 } else { 1.0 },
                transmission_condition: if engine_empty { 1.0 } else { 0.0 },
                ..DriveSupply::WHOLE
            };
            // Fixture: coast up a supported ten-degree slope with the lever released.
            let grade = fp.grip.side_weight * 10.0_f32.to_radians().sin();
            let mut state = TransmissionState {
                omega_e: 0.0,
                ..fresh(&tp)
            };
            let mut speed = 3.0;
            for _ in 0..64 * 10 {
                let mut inp = input(0.0, 0.0, [speed; 2], [grade; 2]);
                inp.supply = supply;
                speed = step(&fp, &tp, &mut state, &inp).next_speeds[0];
            }
            assert!(state.park, "staffing, not task throughput, owns park");
            // Resolution: f32 cancellation at a supported stop.
            assert!(speed.abs() < 1e-5, "released coast must stop: {speed}");
            for command in [-1.0, 1.0] {
                // Fixture: either retained direction at rest.
                state = TransmissionState {
                    omega_e: 0.0,
                    ..fresh(&tp)
                };
                // Fixture: above dynamic capacity but below static breakaway.
                let grade = tp.brake_capacity_n * (1.0 + tp.brake_static_factor) * 0.5;
                let mut inp = input(command, 0.0, [0.0; 2], [grade; 2]);
                inp.supply = supply;
                let report = step(&fp, &tp, &mut state, &inp);
                assert!(state.park);
                // Resolution: f32 cancellation at a supported stop.
                assert!(report.next_speeds.iter().all(|v| v.abs() < 1e-5));
            }
        }
    }
}

/// A restaffed Driver retains the gear during rollback, never buying a cut for
/// backward motoring drag.
#[test]
fn restaffed_rollback_retains_gear_and_refuses_backward_overrev() {
    let (fp, tp) = (grade_fixture_fp(), tiger_tp());
    // Fixture: retained upper-middle forward gear, rollback at 2 m/s on ten degrees.
    let gear = (tp.gears_fwd.len() * 3 / 4) as u8;
    let speed = -2.0;
    let load = fp.grip.side_weight * 2.0 * 10.0_f32.to_radians().sin();
    let mut inp = driver_input(speed, load, 1.0);
    inp.reactions = [load * 0.5; 2];
    let initial = TransmissionState {
        gear,
        clutch: ClutchState::In,
        omega_e: speed * tp.gears_fwd[usize::from(gear - 1)] / tp.sprocket_radius,
        ..fresh(&tp)
    };
    let mut state = initial;
    inp.supply.control_authority = false;

    inp.supply.task_rate = 0.0;
    assert!(!step(&fp, &tp, &mut state, &inp).shifting);
    inp.supply = DriveSupply::WHOLE;
    inp.intent = DriveIntent::Axes(DriveAxes {
        throttle: 1.0,
        steer: 0.0,
    });
    let report = step(&fp, &tp, &mut state, &inp);
    assert!(!report.shifting, "restaffing cannot buy a rollback cut");
    assert_eq!(state.gear, gear);
}

/// Full service during rollback must not buy a gear cut, even while crank inertia
/// still carries forward rotation. The player stops before choosing a launch gear.
#[test]
fn forward_crank_rollback_keeps_gear_under_service() {
    let (fp, tp) = (grade_fixture_fp(), tiger_tp());
    // Fixture: supported twenty-degree rollback in a retained launch gear.
    let load = fp.grip.side_weight * 2.0 * 20.0_f32.to_radians().sin();
    let inp = driver_input(-0.2, load, 1.0);
    let mut state = TransmissionState {
        gear: tp.launch_gears,
        park: false,
        clutch: ClutchState::In,
        omega_e: tp.engine.idle_rpm * RPM_TO_RAD,
        ..fresh(&tp)
    };
    let gear = state.gear;
    let report = step(&fp, &tp, &mut state, &inp);
    assert_eq!(state.gear, gear);
    assert!(!report.shifting);
}

/// Construction drag may stop a free crank but cannot reverse it, including either side
/// of zero. A second rest fade is unnecessary; the player receives no invented crank energy.
#[test]
fn free_crank_drag_stops_without_crossing_zero() {
    for tp in [lab_tp(), tiger_tp()] {
        // Fixture: crank speeds on both sides of rest, near the zero crossing.
        for omega in [-0.01_f32, 0.0, 0.01] {
            let mut inp = input(0.0, 0.0, [0.0; 2], [0.0; 2]);
            inp.supply.task_rate = 0.0;
            #[cfg(feature = "bitprobe")]
            let mut probe = TransmissionProbe::default();
            let (torque, _) = crank_free_torque(
                &tp,
                &inp,
                false,
                false,
                0.0,
                0.0,
                1.0,
                omega,
                tp.engine.idle_rpm * RPM_TO_RAD,
                #[cfg(feature = "bitprobe")]
                &mut probe,
            );
            let next = omega + torque * inp.dt / tp.engine_inertia;
            assert_eq!(
                next, 0.0,
                "free drag must stop, not reverse: {omega} -> {next}"
            );
        }
    }
}

/// The game's driving and damage model: a staffed station can retain its park lever when
/// the engine HP pool is emptied. A steering request with no task work must not release it.
#[test]
fn zero_rate_steering_retains_park_on_a_supported_slope() {
    for (fp, tp) in class_drivetrains() {
        // Fixture: ten-degree supported grade; no stored crank energy can turn the belts.
        let grade = fp.grip.side_weight * 10.0_f32.to_radians().sin();
        let mut state = TransmissionState {
            park: true,
            gear: tp.launch_gears,
            omega_e: 0.0,
            ..fresh(&tp)
        };
        let mut speeds = [0.0; 2];
        for tick in 0..128 {
            let mut inp = input(0.0, if tick < 64 { 1.0 } else { 0.0 }, speeds, [grade; 2]);
            inp.supply.task_rate = 0.0;
            inp.supply.engine_condition = 0.0;
            let report = step(&fp, &tp, &mut state, &inp);
            assert!(state.park, "unworked steering must retain the park lever");
            // Resolution: f32 stop-force cancellation at the supported rest.
            assert!(report.next_speeds.iter().all(|v| v.abs() < 1e-5));
            speeds = report.next_speeds;
        }
    }
}

/// The game's driving and damage model: service brakes belong to the staffed Driver,
/// independently of engine throughput. An empty station retains only construction drag.
#[test]
fn zero_rate_staffed_descent_keeps_the_crank_ceiling_with_brakes() {
    for (fp, tp) in class_drivetrains() {
        // Fixture: upper-middle gear at the ceiling; descent demand exceeds drag by a
        // construction-scaled share of service capacity, so both staffing cases differ.
        let gear = (tp.gears_fwd.len() * 3 / 4).max(1) as u8;
        let k = tp.gears_fwd[usize::from(gear - 1)] / tp.sprocket_radius;
        let ceiling = tp.engine.crank_limit_rpm * RPM_TO_RAD / k;
        let grade = engine_drag(&tp, tp.engine.crank_limit_rpm * RPM_TO_RAD) * k * 0.5
            + tp.brake_capacity_n * 0.25;
        let state = TransmissionState {
            gear,
            park: false,
            clutch: ClutchState::In,
            omega_e: tp.engine.crank_limit_rpm * RPM_TO_RAD,
            ..fresh(&tp)
        };
        let mut inp = input(0.0, 0.0, [ceiling; 2], [-grade; 2]);
        inp.supply.task_rate = 0.0;
        inp.supply.engine_condition = 0.0;
        let staffed = step(&fp, &tp, &mut { state }, &inp);
        // Resolution: f32 integration at a data-derived ceiling.
        assert!(staffed.next_speeds[0] <= ceiling + 1e-5);
        inp.supply.control_authority = false;

        let unstaffed = step(&fp, &tp, &mut { state }, &inp);
        assert!(unstaffed.next_speeds[0] > ceiling);
    }
}

/// A phase hold remains a service brake with an empty station, but it cannot work the
/// retained park lever or refill the crank. The phase gate relies on this independent hold.
#[cfg(feature = "bitprobe")]
#[test]
fn explicit_hold_without_a_driver_retains_construction_rest() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut inp = input(0.0, 0.0, [0.0; 2], [1000.0; 2]); // Fixture: within service capacity.
    inp.intent = DriveIntent::Brake;
    inp.supply.control_authority = false;
    let mut state = TransmissionState {
        park: false,
        omega_e: 0.0,
        ..fresh(&tp)
    };
    let report = step(&fp, &tp, &mut state, &inp);
    assert!(!state.park);
    assert_eq!(state.clutch, ClutchState::In);
    assert_eq!(report.next_speeds, [0.0; 2]);
    assert_eq!(
        [report.bitprobe.tau_induced, report.bitprobe.tau_idle],
        [0.0; 2]
    );
}

/// One lever value must select one radius immediately from every prior detent. Entry and
/// release use the same authored threshold; a retained exit band fails below either threshold.
#[test]
fn steer_detents_use_one_threshold_from_every_history() {
    let feel = super::super::drive::drive_feel();
    for prior in [0, 1, 2] {
        for (intent, expected) in [
            (0.0, 0),
            (feel.wide_on.0.next_down(), 0),
            (feel.wide_on.0, 1),
            (feel.tight_on.0.next_down(), 1),
            (feel.tight_on.0, 2),
            (1.0, 2),
        ] {
            let mut state = TransmissionState {
                steer_step: prior,
                ..fresh(&lab_tp())
            };
            update_steer_detent(&mut state, intent);
            assert_eq!(state.steer_step, expected, "prior={prior}, lever={intent}");
        }
    }
}

/// Keyboard steering enters wide on its first notch and tight only on its last, so the
/// player can retain a wide turn at any intermediate notch in either direction.
#[test]
fn steering_notches_select_wide_until_the_last_selects_tight() {
    let feel = super::super::drive::drive_feel();
    for sign in [-1.0, 1.0] {
        let mut state = fresh(&lab_tp());
        for index in 1..=feel.notch_count.0 {
            let lever = sign * f32::from(index) / f32::from(feel.notch_count.0);
            update_steer_detent(&mut state, lever);
            assert_eq!(
                state.steer_step,
                if index == feel.notch_count.0 { 2 } else { 1 }
            );
        }
    }
}

/// Admission follows algebraic domains and representability, not a vehicle class's scale.
/// Fixture extremes retain finite coupled arithmetic across different physical scales.
#[test]
fn construction_admits_finite_scales_and_refuses_unrepresentable_arithmetic() {
    let mut a = tiger_authoring();
    a.idle_rpm = 25000.0;
    a.governed_rpm = 40000.0;
    a.crank_limit_rpm = 40100.0; // Fixture: completed fuel cut.
    a.rated_rpm = 40000.0;
    a.torque_nm = &[(25000.0, 200000.0), (40000.0, 150000.0)];
    a.brake_static_factor = 3.0;
    a.engine_inertia_kgm2 = (200000.0 / engine_control().idle_catch_droop_rpm.value
        + 200000.0 * a.drag_fraction * (1.0 - motoring_drag_base_share()) / 32500.0)
        / (TICK_HZ * RPM_TO_RAD)
        * 1.001;
    a.clutch_capacity_nm = 100000.0;
    a.belt_inertia = 0.5;
    a.shift_secs = 3.5;
    assert!(TransmissionParams::from_authoring(&a).is_ok());
    a.engine_inertia_kgm2 = 0.01;
    a.clutch_capacity_nm = 50.0;
    assert!(TransmissionParams::from_authoring(&a).is_err());
    a.engine_inertia_kgm2 = (200000.0 / engine_control().idle_catch_droop_rpm.value
        + 200000.0 * a.drag_fraction * (1.0 - motoring_drag_base_share()) / 32500.0)
        / (TICK_HZ * RPM_TO_RAD)
        * 1.001;
    a.shift_secs = (f32::from(u8::MAX) + 1.0) / TICK_HZ;
    assert!(TransmissionParams::from_authoring(&a).is_err());
    // A cut that rounds to no tick would change gear with the clutch in, unfelt by the ledger.
    a.shift_secs = 0.4 / TICK_HZ;
    assert!(TransmissionParams::from_authoring(&a).is_err());
    a.shift_secs = 3.5;
    a.belt_inertia = f32::MIN_POSITIVE;
    assert!(TransmissionParams::from_authoring(&a).is_err());
}

/// The coast expectation must solve the same force balance below and above the rolling
/// fade, on every construction; a full-resistance inverse rejects valid slow roots.
#[test]
#[cfg(feature = "bitprobe")]
fn coast_expectation_includes_the_rolling_fade_for_every_drivetrain() {
    let mut fade_roots = 0;
    for (fp, mut tp) in class_drivetrains() {
        let mut roots = 0;
        for fraction in [tp.drag_fraction, 0.7] {
            // Fixture: includes a slow root on the seed.
            tp.drag_fraction = fraction;
            let weight = fp.grip.side_weight * 2.0;
            let angle = 10.0_f32.to_radians(); // Fixture slope.
            let rolling = crate::ground_types::GroundTypes::load_shipped()
                .get(crate::terrain_blend::BASE_GROUND)
                .contact()
                .rolling_at(fp.grip.nominal_pressure_kpa);
            for gear in 1..=tp.gears_fwd.len() as u8 {
                if let Some(speed) =
                    crate::headless_test::coast_equilibrium(&tp, gear, weight, angle, rolling)
                {
                    roots += 1;
                    fade_roots += usize::from(speed < forces::ROLLING_FADE_SPEED);
                    let k = tp.gears_fwd[usize::from(gear - 1)] / tp.sprocket_radius;
                    let drag = engine_drag(&tp, speed * k) * k;
                    let ground = forces::rolling_resistance(weight * rolling * angle.cos(), speed);
                    // Resolution: independent inverse and forward f32 arithmetic at vehicle-force scale.
                    assert!((drag + ground - weight * angle.sin()).abs() < force_margin(&tp));
                    println!(
                        "MEASURED coast gears={} gear={gear} drag_fraction={fraction} speed={speed}",
                        tp.gears_fwd.len()
                    );
                }
            }
        }
        assert!(
            roots > 0,
            "each Fixture must exercise an unclamped motoring root"
        );
    }
    assert!(
        fade_roots > 0,
        "the expectation must exercise the rolling fade band"
    );
}

/// A backward crank cannot fire or acquire a positive idle floor. Crank settlement relies
/// on the live sign even though candidate pricing may assume a forward free crank.
#[test]
fn backward_live_crank_has_no_idle_floor() {
    for (_, tp) in class_drivetrains() {
        assert!(idle_floor(&tp, DriveSupply::WHOLE, 0.0).is_some());
        let state = TransmissionState {
            clutch: ClutchState::In,
            omega_e: -tp.engine.idle_rpm * RPM_TO_RAD,
            ..fresh(&tp)
        };
        let inp = input(1.0, 0.0, [0.0; 2], [0.0; 2]);
        let [fuel, catch, drag] = engine_torques(&tp, inp.supply, state.omega_e, 1.0);
        let (floor, over) = crank_guards(&tp, &inp, &state, 1.0, fuel + catch - drag);
        assert_eq!(floor, -over);
    }
}

/// A free-crank tick must resolve the slope of its authored torque and fuel cut.
/// The constructor protects every admitted drivetrain, including Blender's fixture sheet.
#[test]
fn unresolved_crank_inertia_is_refused() {
    let mut a = tiger_authoring();
    let raw_slope = a
        .torque_nm
        .windows(2)
        .map(|p| ((p[1].1 - p[0].1) / (p[1].0 - p[0].0)).abs())
        .fold(0.0_f32, f32::max);
    let resolved = TransmissionParams::from_authoring(&a).unwrap();
    let cut_slope =
        resolved.raw_torque_at(a.governed_rpm) / engine_control().governor_cut_rpm.value;
    assert!(
        cut_slope > raw_slope,
        "the fixture distinguishes raw slope from fuel cut"
    );
    a.engine_inertia_kgm2 = (raw_slope + cut_slope) / (2.0 * TICK_HZ * RPM_TO_RAD);
    assert!(TransmissionParams::from_authoring(&a).is_err());
}

/// Virtual free-crank guard clipping must not cap synchronous curve power.
/// This isolates the work law; constructor admission independently bounds the tick's inertia.
#[test]
fn virtual_free_crank_energy_does_not_cap_curve_power() {
    let (fp, mut tp) = (lab_fp(), lab_tp());
    for inertia in [0.01, 4.0] {
        // Fixture: unresolved and resolved crank inertias.
        tp.engine_inertia = inertia;
        let omega = 1100.0 * RPM_TO_RAD; // Fixture curve peak.
        let k = tp.gears_fwd[2] / tp.sprocket_radius;
        let speed = omega / k;
        let torque = tp.torque_at(omega / RPM_TO_RAD);
        let force = torque * k;
        let inp = input(1.0, 0.0, [speed; 2], [force * 0.5; 2]);
        let crank = CrankTick {
            omega_e: omega,
            tau_free: torque,
            k,
            dir: 1.0,
            omega_floor: 500.0 * RPM_TO_RAD,
            omega_over: 1900.0 * RPM_TO_RAD,
            dt: inp.dt,
        };
        #[cfg(feature = "bitprobe")]
        let mut probe = TransmissionProbe::default();
        let work = engine_work(
            &tp,
            &BeltTick::new(&construction(&fp), &inp),
            crank,
            [force * 0.5; 2],
            #[cfg(feature = "bitprobe")]
            &mut probe,
        );
        assert_eq!(
            work.scale, 1.0,
            "curve power must remain available at J={inertia}"
        );
    }
}

/// Reverse steering belongs to reverse construction, even when its ratios differ from forward.
/// The Driver needs that radius before a belt moves.
#[test]
fn reverse_radii_belong_to_the_reverse_gear() {
    let mut a = tiger_authoring();
    a.forward_speeds_kmh = &[5.0, 12.0];
    a.reverse_speeds_kmh = &[10.0]; // Fixture: not a forward prefix.
    a.reverse_steer_radii_m = &[(6.0, 16.0)];
    a.steer_radii_m = &[(3.0, 8.0), (7.0, 19.0)];
    let tp = TransmissionParams::from_authoring(&a).unwrap();
    let state = TransmissionState {
        reverse: true,
        steer_step: 2,
        ..fresh(&tp)
    };
    assert_eq!(tp.steering_radii(true, 1).0, 6.0);
    let kappa = a.half_tread_m / 6.0; // DERIVED from the Fixture reverse radius.
    assert_eq!(detent_kappa(&tp, &state, 1), kappa);
    let fp = grade_fixture_fp();
    let mut inp = input(0.0, 1.0, [-1.0; 2], [0.0; 2]); // Fixture: reverse travel.
    inp.dt = 1.0 / TICK_HZ;
    let (_, _, jacobian) = steering_force(
        &tp,
        &BeltTick::new(&construction(&fp), &inp),
        &state,
        &inp,
        -1.0,
        0.0,
        None,
    );
    assert_eq!(jacobian, [(1.0 + kappa) / 2.0, -(1.0 - kappa) / 2.0]);
    assert_eq!(
        crate::drive_hud::steering_hud_line(&state, Some(&tp)),
        "STEER II R~6m"
    );
}

/// Finite authored scalars must not publish an overflowing crank-energy budget to the work law.
#[test]
fn overflowing_crank_energy_is_refused_before_play() {
    let mut a = tiger_authoring();
    // Fixture: finite inertia and angular speed whose energy overflows f32, with tiny reductions.
    a.idle_rpm = 1e7;
    a.governed_rpm = 1e8;
    a.crank_limit_rpm = 2e8; // Fixture: finite crank whose energy overflows.
    a.rated_rpm = 1e8;
    a.torque_nm = &[(1e7, 1.0), (1e8, 1.0)];
    a.engine_inertia_kgm2 = 1e25;
    a.forward_speeds_kmh = &[1e25, 2e25];
    a.reverse_speeds_kmh = &[1e25];
    a.reverse_steer_radii_m = &[(6.0, 16.0)];
    a.steer_radii_m = &[(3.0, 8.0), (7.0, 19.0)];
    let error = TransmissionParams::from_authoring(&a)
        .unwrap_err()
        .to_string();
    assert!(error.contains("crank energy"), "{error}");
}

/// A cut's landing must use this hull's mass and both belt inertias on every construction.
#[test]
fn coupled_mass_uses_each_constructions_own_weight() {
    for (fp, _) in class_drivetrains() {
        let expected = 2.0 * fp.grip.side_weight / super::super::derive::G + 2.0 * fp.inertia;
        assert_eq!(
            coupled_mass(&construction(&fp)),
            expected,
            "landing mass belongs to this construction"
        );
    }
}

/// A curve extension at its end-clamped torque changes no delivered torque or crank limit.
/// Ceiling brakes, over-rev and force roundoff must retain their envelope.
#[test]
fn a_curve_tail_does_not_move_the_crank_envelope() {
    let original = tiger_authoring();
    let base = TransmissionParams::from_authoring(&original).unwrap();
    let mut curve = original.torque_nm.to_vec();
    let &(rpm, torque) = curve.last().unwrap();
    curve.push((rpm + 500.0, torque)); // Fixture: extend the end-clamped tail.
    let extended = TransmissionParams::from_authoring(&TransmissionAuthoring {
        torque_nm: &curve,
        ..original
    })
    .unwrap();
    for rpm in [600.0, 2100.0, 2500.0, 3000.0, 3500.0] {
        // Fixture samples.
        assert_eq!(base.torque_at(rpm), extended.torque_at(rpm));
    }
    assert_eq!(base.engine.crank_limit_rpm, extended.engine.crank_limit_rpm);
    assert_eq!(base.roundoff_force_scale(), extended.roundoff_force_scale());
    let fp = grade_fixture_fp();
    let construction = construction(&fp);
    let gear = base.gears_fwd.len() as u8;
    let k = base.gears_fwd[usize::from(gear - 1)] / base.sprocket_radius;
    let omega = base.engine.crank_limit_rpm * RPM_TO_RAD;
    let mut state = TransmissionState {
        gear,
        clutch: ClutchState::In,
        omega_e: omega,
        ..fresh(&base)
    };
    let inp = driver_input(omega / k, 0.0, 0.0);
    let guards = crank_guards(&base, &inp, &state, 0.0, 0.0);
    assert_eq!(guards, crank_guards(&extended, &inp, &state, 0.0, 0.0));
    let controls = driver(&construction, &base, &mut state, &inp);
    let crank = CrankTick {
        omega_e: omega,
        tau_free: 0.0,
        k,
        dir: 1.0,
        omega_floor: guards.0,
        omega_over: guards.1,
        dt: inp.dt,
    };
    let brake_forces = |tp: &TransmissionParams| {
        let mut q = [10_000.0; 2]; // Fixture: force that would accelerate past the limit.
        let belts = BeltTick::new(&construction, &controls.input);
        apply_ceiling_brakes(&mut q, tp, &belts, &state, &controls, crank);
        q
    };
    assert_eq!(brake_forces(&base), brake_forces(&extended));
    assert!(
        brake_forces(&base)[0] < 10_000.0,
        "the witness reaches ceiling braking"
    );
}

/// Admission refuses a live fuel rack at the do-not-exceed limit before any Driver can brake it.
#[test]
fn the_crank_limit_requires_completed_fuel_cut() {
    let mut a = tiger_authoring();
    a.crank_limit_rpm = a.governed_rpm;
    let error = TransmissionParams::from_authoring(&a)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("engine.crank_limit_rpm") && error.contains("fuel cut"),
        "{error}"
    );
    a.crank_limit_rpm += engine_control().governor_cut_rpm.value;
    assert!(TransmissionParams::from_authoring(&a).is_ok());
}

/// Admission follows the torque law's slope, not how many samples the author used.
/// A collinear point must not force a construction fact to change.
#[test]
fn a_collinear_knot_does_not_change_crank_admission() {
    let original = tiger_authoring();
    let base = TransmissionParams::from_authoring(&original).unwrap();
    let mut curve = original.torque_nm.to_vec();
    curve.insert(1, (826.0, 1311.0)); // Fixture: 26 rpm and 11 N m on the first line.
    let refined = TransmissionParams::from_authoring(&TransmissionAuthoring {
        torque_nm: &curve,
        ..original
    })
    .expect("a collinear knot must leave the inertia admission unchanged");
    for rpm in [800.0, 826.0, 1000.0, 1500.0, 2100.0, 2550.0] {
        // Fixture samples.
        assert!((base.torque_at(rpm) - refined.torque_at(rpm)).abs() < 0.001);
    }
}

/// A genuinely steep torque branch must still be refused at the fixed tick; authors must
/// correct physical inputs or the solver resolution, rather than change curve sampling.
#[test]
fn a_steep_torque_branch_is_refused_by_its_slope() {
    let original = tiger_authoring();
    let mut curve = original.torque_nm.to_vec();
    curve.insert(1, (810.0, 1850.0)); // Fixture: a genuinely steep 55 N m/rpm branch.
    let error = TransmissionParams::from_authoring(&TransmissionAuthoring {
        torque_nm: &curve,
        ..original
    })
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("torque slope") && error.contains("inertia_kgm2"),
        "{error}"
    );
}

/// A raw peak beyond fuel cut is unavailable to launch. Both classes must select a capable
/// gear inside their declared launch domain instead of retaining an out-of-domain cruise gear.
#[test]
fn launch_targets_the_peak_available_after_fuel_cut() {
    for mut spec in [
        super::super::drive_fixture::compact_spec(),
        super::super::rig_geom::tiger_spec(),
    ] {
        let tr = &mut spec.track.powertrain.transmission;
        let peak = tr
            .engine
            .torque_curve
            .iter()
            .map(|point| point.1)
            .fold(0.0_f32, f32::max);
        // Fixture: compact's tail becomes 900 N m at 2600 rpm, beyond its 2200 rpm governor.
        tr.engine.torque_curve.last_mut().unwrap().1 = peak * 1.125;
        let mut fp = lab_fp();
        fp.inertia = 3000.0; // Fixture arithmetic coupling.
        fp.grip.side_weight = spec.mass * super::super::derive::G / 2.0;
        let tp = tr.params(0.3, 1.0, fp.inertia).unwrap(); // Fixture sprocket radius and half-tread.
        let inp = driver_input(0.0, 0.0, 1.0);
        let selected = affordable_launch(&tp, &inp, 1.0)
            .expect("fuel-governed peak must select a capable launch");
        assert!(tp.torque_at(tp.launch_omega(1.0) / RPM_TO_RAD) > 0.0);
        let mut state = TransmissionState {
            gear: tp.gears_fwd.len() as u8,
            park: true,
            ..fresh(&tp)
        };
        choose_gear(&mut state, &tp, &construction(&fp), &inp, 1.0, felt(&inp));
        assert_eq!(state.gear, selected);
    }
}

/// The launch target follows the interior maximum of raw torque times fuel cut, not a knot
/// or a clamp to governed rpm. Fixture: (100 + 9*x)*(1 - x/100) on the fuel ramp.
#[test]
fn launch_peak_can_lie_inside_the_fuel_cut_ramp() {
    let mut a = tiger_authoring();
    a.governed_rpm = 1000.0;
    a.torque_nm = &[
        (800.0, 100.0),
        (1000.0, 100.0),
        (1100.0, 1000.0),
        (3000.0, 1000.0),
    ];
    let tp = TransmissionParams::from_authoring(&a).unwrap();
    let expected = 1000.0 + 8.0 / 0.18; // DERIVED vertex of 100 + 8*x - 0.09*x^2.
    assert!((tp.peak_torque_rpm - expected).abs() < 0.01); // Resolution: f32 rpm interpolation.
    assert!(tp.torque_at(tp.peak_torque_rpm) > tp.torque_at(a.governed_rpm));
}

/// Admission must include the steep idle catch, independently of the high-rpm fuel ramp.
/// Fixture: a compact sheet at low inertia; operators must get the control widths in its refusal.
#[test]
fn construction_refuses_an_unresolved_idle_catch() {
    let spec = super::super::drive_fixture::compact_spec();
    let tr = &spec.track.powertrain.transmission;
    let mut params = tr.params(0.3, 1.0, 3000.0).unwrap();
    let control = engine_control();
    let drag_slope =
        params.peak_torque_nm * params.drag_fraction * (1.0 - motoring_drag_base_share())
            / ((params.engine.idle_rpm + params.engine.governed_rpm) * 0.5);
    let idle_slope = params.torque_at(params.engine.idle_rpm) / control.idle_catch_droop_rpm.value;
    // DERIVED for a falling linear net-curve span times linear fuel cut: the largest
    // derivative magnitude is at cut start, T(governed)/width - curve_slope.
    let pair = params
        .engine
        .torque_nm
        .windows(2)
        .find(|pair| {
            pair[0].0 <= params.engine.governed_rpm && pair[1].0 > params.engine.governed_rpm
        })
        .unwrap();
    let curve_slope = (pair[1].1 - pair[0].1) / (pair[1].0 - pair[0].0);
    assert!(curve_slope < 0.0, "fixture isolates a falling cut span");
    let cut_slope =
        params.torque_at(params.engine.governed_rpm) / control.governor_cut_rpm.value - curve_slope;
    let cut_end = params.engine.governed_rpm + control.governor_cut_rpm.value;
    let loss_cut_slope =
        engine_drag(&params, cut_end * RPM_TO_RAD).abs() / control.governor_cut_rpm.value;
    assert!(
        idle_slope > cut_slope + loss_cut_slope,
        "fixture isolates the idle branch"
    );
    let minimum = (idle_slope + drag_slope) / (TICK_HZ * RPM_TO_RAD);
    println!(
        "DERIVED compact inertia minimum={minimum:.8} without_idle={:.8} without_drag={:.8}",
        (cut_slope + loss_cut_slope + drag_slope) / (TICK_HZ * RPM_TO_RAD),
        idle_slope / (TICK_HZ * RPM_TO_RAD)
    );
    for without in [cut_slope + loss_cut_slope + drag_slope, idle_slope] {
        // Midpoints isolate idle_catch and viscous drag independently of every other term.
        params.engine_inertia = (minimum + without / (TICK_HZ * RPM_TO_RAD)) * 0.5;
        let error = params.check_derived(3000.0).unwrap_err().to_string();
        assert!(error.contains("idle_catch_droop_rpm") && error.contains("governor_cut_rpm"));
    }
    params.engine_inertia = minimum * 1.001; // Resolution: avoid rounding at the admitted boundary.
    params.check_derived(3000.0).unwrap();

    // Fixture: a constant net curve isolates the loss cut; its falling slope is T/cut_width.
    params.engine.torque_nm = vec![(700.0, 100.0), (1400.0, 800.0), (2600.0, 800.0)];
    let net_slope = 800.0 / control.governor_cut_rpm.value;
    let loss_cut_slope =
        engine_drag(&params, cut_end * RPM_TO_RAD).abs() / control.governor_cut_rpm.value;
    let without = (net_slope + drag_slope) / (TICK_HZ * RPM_TO_RAD);
    let with = (net_slope + loss_cut_slope + drag_slope) / (TICK_HZ * RPM_TO_RAD);
    println!("DERIVED fuel-cut fixture inertia minimum={with:.8} without_loss_cut={without:.8}");
    params.engine_inertia = (with + without) * 0.5;
    assert!(
        params.check_derived(3000.0).is_err(),
        "loss cut must participate in admission"
    );
    params.engine_inertia = with * 1.001;
    params.check_derived(3000.0).unwrap();
}
