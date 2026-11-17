use super::super::drive::DriveAxes;
use super::super::drive_fixture::{step, *};
use super::super::driver::Sensed;
use super::super::forces;
use super::*;

/// Fixture: the Driver has felt a hull at rest meeting nothing, on ground that grips beyond any
/// gear's pull. From an open clutch it launches in its highest launch gear; once engaged, no gear
/// launches with more than the one it holds, so no gear changes.
fn felt_at_rest(mut inp: TransmissionInput) -> TransmissionInput {
    inp.sensed = Some(Sensed::steady(0.0, 0.0, f32::INFINITY));
    inp
}

/// Fixture: `felt_at_rest` with belts that meet no ground, which gives them no grip to carry a
/// pull, so no gear offers more than the gear engaged.
fn felt_free_at_rest(mut inp: TransmissionInput) -> TransmissionInput {
    inp.sensed = Some(Sensed::steady(0.0, 0.0, 0.0));
    inp
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
        launch: LaunchState::Locked,
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
        launch: LaunchState::Locked,
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
        launch: LaunchState::Locked,
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
    let initial_speed = 6.0;
    let load = 5_000.0;
    let fixture = input(0.0, 0.0, [initial_speed; 2], [-load; 2]);
    // DERIVED: per-side momentum divided by net brake impulse per tick, rounded up;
    // one further tick lets the Driver latch park after the belt lands at rest.
    let stop_deadline = (fp.inertia * initial_speed / ((tp.brake_capacity_n - load) * fixture.dt))
        .ceil() as usize
        + 1;
    // DERIVED: one stopping window followed by an equally long parked window.
    let trace_budget = 2 * stop_deadline;
    let run = |brake: bool| {
        let mut st = TransmissionState {
            gear: 4,
            ..fresh(&tp)
        };
        let mut speeds = fixture.speeds;
        let mut trace = Vec::new();
        for _ in 0..trace_budget {
            let inp = TransmissionInput {
                supply: DriveSupply::WHOLE,
                intent: if brake {
                    DriveIntent::Brake
                } else {
                    DriveIntent::Axes(DriveAxes::default())
                },
                speeds,
                ..fixture
            };
            speeds = step(&fp, &tp, &mut st, &inp).next_speeds;
            trace.push((speeds, st.park));
        }
        trace
    };
    let held = run(true);
    let stopped = held
        .iter()
        .position(|(speeds, park)| *park && speeds.iter().all(|v| v.abs() < park_engage_speed()))
        .expect("the brake hold must bring the tank to a parked rest");
    assert!(
        stopped < stop_deadline,
        "the brake hold took {stopped} ticks to park; derived deadline {stop_deadline}"
    );
    for (tick, (speeds, park)) in held.iter().enumerate().skip(stopped) {
        assert!(
            *park && speeds.iter().all(|v| v.abs() < park_engage_speed()),
            "tick {tick}: a held tank stays parked, got {speeds:?}"
        );
    }
    assert!(
        held.iter()
            .all(|(speeds, _)| speeds.iter().all(|v| *v > -park_engage_speed())),
        "the brake hold never drives the belt through zero"
    );
    let coasting = run(false);
    let (speeds, _) = coasting[stopped];
    assert!(
        speeds.iter().all(|v| *v > 1.0),
        "a released command coasts: the hold must be the brakes, got {speeds:?}"
    );
}

/// Releasing steer centers the steering row at zero belt difference. With no
/// ground reactions the row alone must arrest the pivot, so the driver's released stick
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
    assert!(d0 > 0.0, "the fixture must actually pivot (d = {d0})");
    // The difference row applies half its impulse to each belt: stopping impulse is 2 I d.
    let stop_ticks = (2.0 * fp.inertia * d0
        / (tp.steer_capacity_n * input(0.0, 0.0, speeds, [0.0; 2]).dt))
        .ceil() as usize
        + 1;
    let mut previous = d0;
    for _ in 0..stop_ticks {
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

/// A parked stop on unequal ground shares load through the neutral differential. The total
/// fits both brakes but exceeds one side's static capacity; independent stops must fail here.
#[test]
fn two_parked_brakes_share_a_neutral_stop_from_exact_rest() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let static_cap = tp.brake_capacity_n * tp.brake_static_factor;
    // Fixture: uneven ground pulls, with their mean inside the pair's capacity.
    let mut inp = input(0.0, 0.0, [0.0; 2], [1.15 * static_cap, 0.65 * static_cap]);
    inp.reaction_slopes = [0.05, 0.8].map(|share| share * fp.inertia / inp.dt);
    let mut state = fresh(&tp);
    let report = step(&fp, &tp, &mut state, &inp);
    assert_eq!(
        report.next_speeds, [0.0; 2],
        "the differential must share the parked load"
    );
    assert_eq!(
        report.held, [true; 2],
        "both parked surfaces must report rest"
    );
    assert!(!report.brakes_slide);
    // Fixture: the same construction with insufficient steering transfer cannot share the load.
    let limited = TransmissionParams {
        steer_capacity_n: 0.01 * static_cap,
        ..tp
    };
    let report = step(&fp, &limited, &mut fresh(&limited), &inp);
    assert!(
        report.next_speeds.iter().any(|v| *v != 0.0),
        "sharing must respect steering capacity"
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
    let feel = super::super::drive::drive_feel();
    let wide = 0.5 * (feel.wide_on.0 + feel.tight_on.0);
    let rep = step(
        &fp,
        &tp,
        &mut st,
        &input(1.0, wide, [cruise_m, cruise_m], [0.0, 0.0]),
    );

    assert_eq!(
        rep.steer_step, 1,
        "a steer between the thresholds engages the WIDE detent"
    );
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

/// The player must not get reversed steering when equal ground reactions take the mean
/// through rest. Fixture: a paid interruption and sufficient reaction to force rollback.
#[test]
fn steering_does_not_reverse_when_the_mean_crosses_rest() {
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

/// The parking brake settles creep instead of freezing it. Using `B = clamp(R − Q, ±cap)`
/// at a small positive belt speed with `R > Q` would set
/// `v̇ = 0` exactly — positive brake work cancelling grip and drag, preserving creep
/// forever. The stop-force law lands the belt at zero.
#[test]
fn parking_brake_settles_creep() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    // Fixture: input speeds in m/s and ground reactions in N.
    let rep = step(
        &fp,
        &tp,
        &mut st,
        &input(0.0, 0.0, [0.03, 0.03], [20_000.0, 20_000.0]),
    );
    assert!(st.park, "zero command near standstill must latch the park");
    for v in rep.next_speeds {
        assert_eq!(
            v, 0.0,
            "a stuck brake supplies an exactly stationary surface"
        );
    }
}

/// A capacity breach retains the lever and its dynamic resistance at every slide speed.
/// Static breakaway capacity applies only when a belt starts exactly at rest.
#[test]
fn parking_brake_stays_saturated_past_breach() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    // Fixture: exceed static capacity by a positive tenth of the dynamic rating.
    let breach_margin = 0.1 * tp.brake_capacity_n;
    let r_breach = tp.brake_static_factor * tp.brake_capacity_n + breach_margin;
    let mut speeds = [0.0f32; 2];
    let mut last = TransmissionReport::default();
    for _ in 0..30 {
        let inp = input(0.0, 0.0, speeds, [r_breach, r_breach]);
        last = step(&fp, &tp, &mut st, &inp);
        assert!(!last.driving, "released clutch cannot supply drive");
        speeds = last.next_speeds;
    }
    assert!(
        st.park,
        "the latch must not release without a drive command"
    );
    assert!(
        speeds[0] < -park_engage_speed(),
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
    let moving = input(0.0, 0.0, [-(park_engage_speed() * 2.0); 2], [r_breach; 2]);
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

/// Rev-match across an upshift: the crank is continuous through the window, no tick moving it
/// further than its free torque and the clutch's capacity can, `(|τ_free| + τ_cap)·dt/J` (DERIVED
/// from `J dω/dt = τ_free − τ_c` with `|τ_c| ≤ τ_cap`), and once the window closes the held clutch
/// keeps the crank on the geared speed of the belt the step integrated. How far the window's end
/// leaves the crank from the new gear's speed is an observation.
#[test]
fn rev_match_across_upshift_is_continuous() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let g1 = tp.gears_fwd[0];
    let g2 = tp.gears_fwd[1];
    let v_warm = 1_600.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
    let v_up = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
    let mut st = TransmissionState {
        launch: LaunchState::Locked,
        ..fresh(&tp)
    };
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
    let target_rpm = v_up * g2 / tp.sprocket_radius / RPM_TO_RAD;
    let mut window_end_gap = None;
    let mut ticks_since_window = 0u32;
    let mut rep = TransmissionReport::default();
    for tick in 0..96 {
        let inp = input(1.0, 0.0, [v_up, v_up], [0.0, 0.0]);
        let before = st.omega_e;
        rep = step(&fp, &tp, &mut st, &inp);
        // Fuel at full pedal bounds the free torque's drive, and drag its loss.
        let [fuel, catch, drag] = engine_torques(&tp, inp.supply, before, 1.0);
        let slew = (fuel + catch + drag + tp.clutch_capacity) * inp.dt / tp.engine_inertia;
        // Resolution: the integration's f32 rounding at the crank's speed.
        let rounding = 4.0 * f32::EPSILON * before.abs();
        assert!(
            (st.omega_e - before).abs() <= slew + rounding,
            "tick {tick}: the crank moved {before} -> {} rad/s, past {slew} rad/s",
            st.omega_e
        );
        let rpm = st.omega_e / RPM_TO_RAD;
        if st.gear == 2 && !rep.shifting && window_end_gap.is_none() {
            window_end_gap = Some((rpm - target_rpm).abs());
        }
        if window_end_gap.is_some() {
            ticks_since_window += 1;
        }
    }
    assert_eq!(st.gear, 2, "the upshift must have committed");
    let gap = window_end_gap.expect("the window must end inside the run");
    println!(
        "MEASURED rev match: {gap:.0} rpm from the geared landing ({target_rpm:.0}) at the window's end"
    );
    assert!(
        ticks_since_window > 16,
        "post-window settling must be observed"
    );
    // The lock's anchor is the belt the step integrated: this harness holds the input speeds, so
    // the integrated belt rides `k·τ_free·dt/I_m` above them.
    let m_next = (rep.next_speeds[0] + rep.next_speeds[1]) / 2.0;
    let locked = g2 / tp.sprocket_radius * m_next;
    assert!(rep.locked, "the engaged clutch holds the crank");
    // Resolution: the lock's product rounds once.
    assert!(
        (st.omega_e - locked).abs() <= f32::EPSILON * locked,
        "the held crank turns at {} rad/s against the belt's geared {locked} rad/s",
        st.omega_e
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

/// The game's damage model: under a full command the clutch latches at any positive task rate
/// (intent thresholds read the command, never the rate), and a half rate pulls away more weakly
/// than a whole one. Full reverse while rolling brakes at full service capacity through `step`
/// whatever the rate: brakes are construction. A step that scales the service brakes, or a
/// command threshold that turns a small rate into no drive, fails here. Fixture rates are
/// 30/336, 0.5 and 1; the belts run free of ground reactions.
#[test]
fn reduced_supply_pulls_away_and_retains_full_service_brakes() {
    let fp = lab_fp();
    let tp = lab_tp();
    let reduced = DriveSupply {
        task_rate: 30.0 / 336.0,
        ..DriveSupply::WHOLE
    };
    let speeds = [1.0, 0.5, reduced.task_rate].map(|rate| {
        let mut state = fresh(&tp);
        let mut speeds = [0.0; 2];
        for _ in 0..64 {
            let mut full = felt_at_rest(input(1.0, 0.0, speeds, [0.0; 2]));
            full.supply.task_rate = rate;
            let report = step(&fp, &tp, &mut state, &full);
            speeds = report.next_speeds;
            assert!(
                state.clutch != ClutchState::Out || report.shifting,
                "rate {rate}: the clutch transmits outside shift windows"
            );
        }
        speeds[0]
    });
    assert!(
        0.0 < speeds[1] && speeds[1] < speeds[0],
        "a half rate pulls away weakly: {speeds:?}"
    );

    let rolling = TransmissionState {
        park: false,
        ..fresh(&tp)
    };
    let mut reverse = input(-1.0, 0.0, [10.0; 2], [0.0; 2]);
    let (mut whole_state, mut braked_state) = (rolling, rolling);
    let whole = step(&fp, &tp, &mut whole_state, &reverse);
    reverse.supply = reduced;
    let braked = step(&fp, &tp, &mut braked_state, &reverse);
    assert_eq!(
        braked.forces, whole.forces,
        "at idle with no fueling, a reduced task rate changes nothing the brakes or drag do"
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
    inp.supply.engine_in_action = false;
    inp.supply.task_rate = 0.0;
    let rep = step(&fp, &tp, &mut st, &inp);
    assert_eq!(rep.next_speeds, [0.0; 2]);
}

/// The game's damage model: the crank and Driver price the same full-pedal torque at a reduced
/// task rate, including construction drag that does not scale with the rate.
#[test]
fn drag_release_and_driver_read_the_command() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = TransmissionState {
        launch: LaunchState::Locked,
        ..fresh(&tp)
    };
    st.park = false;
    st.omega_e = 1550.0 * RPM_TO_RAD; // Fixture crank operating point above idle.
    let ratio = tp.gears_fwd[0];
    let speed = st.omega_e * tp.sprocket_radius / ratio;
    let mut inp = input(1.0, 0.0, [speed; 2], [0.0; 2]);
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
        engine_in_action: false,
        task_rate: 0.0,
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
            pivoted |= speeds[0] > park_engage_speed() && speeds[1] < -park_engage_speed();
            if pivoted {
                assert!(!st.park);
                assert!(!rep.driving);
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
                inp.supply.engine_in_action = false;
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
    inp.supply.engine_in_action = false;
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
        engine_in_action: false,
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

/// The game's driving model: an engaged gear with its clutch held holds the most its pedal nets at
/// any crank speed down to the idle guard, since the hull slows into it; one slipping below its
/// launch speed holds its slip pull. The engaged gear's verdict relies on both. Mutant: pricing
/// only where the gear runs misses the torque peak below it. Fixture: each class drivetrain's top
/// gear at a third of the pedal, running at its governed speed and slipping at half its launch
/// speed.
#[test]
fn an_engaged_gear_holds_the_most_its_pedal_nets_at_or_below_where_it_runs() {
    let command = 0.3; // Fixture: part pedal.
    for (_, tp) in class_drivetrains() {
        let ratio = *tp.gears_fwd.last().expect("a ladder");
        let at = |omega: f32| omega * tp.sprocket_radius / ratio;
        let pedal = |rpm: f32| {
            let [fuel, _, drag] =
                engine_torques(&tp, DriveSupply::WHOLE, rpm * RPM_TO_RAD, command);
            (fuel - drag) * ratio / tp.sprocket_radius
        };
        let governed = tp.engine.governed_rpm;
        let held = viable_pull(
            &tp,
            DriveSupply::WHOLE,
            at(governed * RPM_TO_RAD),
            ratio,
            command,
            ClutchState::In,
        );
        assert!(
            held >= pedal(tp.peak_torque_rpm) && pedal(tp.peak_torque_rpm) > pedal(governed),
            "running at {governed} rpm the gear holds its peak below: {held} N"
        );
        let slipping = viable_pull(
            &tp,
            DriveSupply::WHOLE,
            at(0.5 * tp.launch_omega(command)),
            ratio,
            command,
            ClutchState::Slip,
        );
        let slip = price_driver_force(
            &tp,
            DriveSupply::WHOLE,
            0.0,
            ratio,
            command,
            ClutchState::Slip,
        );
        assert!(
            (slipping - slip).abs() <= 1e-5 * slip.abs(),
            "below its launch speed the gear holds its slip pull: {slipping} N against {slip} N"
        );
    }
}

/// The game's driving and damage model: a gear whose clutch closes from out pulls what its pedal
/// nets at the crank speed it lands on, or slips at the launch rpm below its launch speed, the
/// clutch's capacity capping the torque either way, and no gear is priced whose crank would land
/// past the engine's crank limit, so the Driver opens no cut toward one forecast to land there.
/// Every Driver clause admits a gear by this price. Mutants: dropping the bound prices a gear
/// landing past the limit; bounding at governed speed or completed fuel cut refuses one landing
/// inside it; crediting the most the pedal nets at a slower crank prices a landing past the torque
/// peak at that peak; capping only the pedal's positive torque prices drag a reduced clutch cannot
/// carry. Fixture: every forward gear of each class drivetrain at full pedal, landing a whole rpm
/// inside the limit, where each pedal nets only drag, a whole rpm past it and at half its launch
/// speed, with the transmission whole and with its HP pool emptied.
#[test]
fn a_gear_lands_where_its_clutch_closes_and_never_past_the_crank_limit() {
    let command = 1.0; // Fixture: full pedal.
    let emptied = DriveSupply {
        transmission_condition: 0.0,
        ..DriveSupply::WHOLE
    };
    for (_, tp) in class_drivetrains() {
        let limit = tp.engine.crank_limit_rpm;
        for supply in [DriveSupply::WHOLE, emptied] {
            let capacity = supply.transmission_condition * tp.clutch_capacity;
            for &ratio in &tp.gears_fwd {
                let lands = |rpm: f32| {
                    let at = rpm * RPM_TO_RAD * tp.sprocket_radius / ratio;
                    landing_pull(&tp, supply, at, ratio, command)
                };
                let inside = limit - 1.0;
                let [fuel, _, drag] = engine_torques(&tp, supply, inside * RPM_TO_RAD, command);
                let nets = (fuel - drag).clamp(-capacity, capacity) * ratio / tp.sprocket_radius;
                let landed = lands(inside).expect("a gear landing inside the crank limit engages");
                assert!(
                    (landed - nets).abs() <= 1e-5 * nets.abs(),
                    "landing at {inside} rpm the gear pulls what its pedal nets there, capped: \
                     {landed} N against {nets} N"
                );
                assert_eq!(lands(limit + 1.0), None, "landing past {limit} rpm");
                let launch = tp.launch_omega(command) / RPM_TO_RAD;
                let slip = price_driver_force(&tp, supply, 0.0, ratio, command, ClutchState::Slip);
                let slipping = lands(0.5 * launch).expect("a slipping clutch holds the launch rpm");
                assert!(
                    (slipping - slip).abs() <= 1e-5 * slip.abs(),
                    "below its launch speed the gear slips at its launch pull: {slipping} N against \
                     {slip} N"
                );
            }
        }
    }
}

/// The game's damage model: commanded work consumes one task rate even when
/// local module condition differs. Fixture: a reduced transmission and a further task bottleneck.
#[test]
fn commanded_work_consumes_the_task_rate_once() {
    let tp = lab_tp();
    let supply = DriveSupply {
        task_rate: 0.3,
        transmission_condition: 0.75,
        ..DriveSupply::WHOLE
    };
    let omega = 1550.0 * RPM_TO_RAD;
    let [whole, ..] = engine_torques(&tp, DriveSupply::WHOLE, omega, 1.0);
    let [applied, ..] = engine_torques(&tp, supply, omega, 1.0);
    assert!((applied / whole - supply.task_rate).abs() < 1e-6);
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
        launch: LaunchState::Locked,
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
                launch: LaunchState::Locked,
                ..fresh(&tp)
            };
            let mut inp = input(throttle, steer, speeds, [0.0; 2]);
            inp.supply.control_authority = authority;

            inp.supply.task_rate = 0.0;
            let mut last = TransmissionReport::default();
            for _ in 0..64 * 4 {
                last = step(&fp, &tp, &mut st, &inp);
            }
            #[cfg(feature = "bitprobe")]
            assert_eq!(
                [last.bitprobe.tau_induced, last.bitprobe.tau_idle],
                [0.0; 2]
            );
            assert!(
                !last.driving,
                "zero task rate cannot send clutch propulsion"
            );
        }
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
                .all(|v| v.is_finite() && v > park_engage_speed())
        );
    }
}

/// Pricing and delivery share the governed net curve, launch rpm and clutch cap. Fixture
/// alterations independently expose a missing cap, launch-speed substitution or governor.
#[test]
fn driver_force_price_uses_the_clutch_outcome_and_capacity() {
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
            launch: LaunchState::TakingUp,
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
    let mut state = TransmissionState {
        launch: LaunchState::Locked,
        ..fresh(&tp)
    };
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
            launch: LaunchState::Locked,
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
            speeds.into_iter().all(|speed| speed > park_engage_speed()),
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
        let was_rest = speeds.iter().all(|v: &f32| v.abs() < park_engage_speed());
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
    assert!(speeds.iter().all(|v| v.abs() < park_engage_speed()));
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
                        speed >= -park_engage_speed(),
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
                        speed > park_engage_speed(),
                        "every capable notch must sustain travel"
                    );
                }
            }
        }
        assert!(
            capable_notches > 0,
            "each class must exercise capable partial-pedal travel"
        );
    }
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
            let hold = (driver_actuation().reversal_hold_s.value / driver_input(0.0, 0.0, 0.0).dt)
                .ceil() as u8;
            for _ in 0..64 * 30 {
                let at_rest = speeds.iter().all(|v| v.abs() < park_engage_speed());
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
                            .all(|v| *v * dir >= -park_engage_speed())
                    );
                }
                speeds = report.next_speeds;
                if state.reverse != reverse
                    && !report.shifting
                    && speeds.iter().all(|v| *v * dir < -park_engage_speed())
                {
                    break;
                }
            }
            println!(
                "MEASURED reversal hold_ticks={} cut_ticks={cut_ticks} cuts={cuts} final_speed={}",
                held_ticks - 1,
                speeds[0]
            );
            assert!(state.reverse != reverse && speeds[0] * dir < -park_engage_speed());
            assert_eq!(cuts, 1);
            assert_eq!(cut_ticks, usize::from(tp.shift_ticks.get()));
        }
    }
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
                !report.shifting || speed >= park_engage_speed(),
                "a launch from rest pays no cut"
            );
            cuts += usize::from(report.shifting && !last_cut);
            last_cut = report.shifting;
            speed = report.next_speeds[0];
            minimum = minimum.min(speed);
            assert!(
                report
                    .next_speeds
                    .iter()
                    .all(|v| *v >= -park_engage_speed())
            );
        }
        println!(
            "MEASURED S2 gears={} belt_kg={} launch={launch} gear={} spare_n={spare} load_n={load} cuts={cuts} min_speed={minimum} final_speed={speed}",
            tp.gears_fwd.len(),
            fp.inertia,
            state.gear
        );
        assert!(speed > park_engage_speed());
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
                reversal_hold_ticks: (driver_actuation().reversal_hold_s.value
                    * (1.0 / fixed_step_s()))
                .ceil() as u8,
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
                } else if speed * command > park_engage_speed() {
                    break;
                }
            }
            assert_eq!(state.reverse, command < 0.0);
            assert_eq!(cuts, 1);
            assert!(speed * command > park_engage_speed());
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
        let speed = (idle_speed + park_engage_speed()) * 0.5;
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
/// where the ladder permits it, with a drive request whose fuel nets nothing at any crank
/// ([`braking_pedal`]), so no cut buys drive and the gear holds.
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
            let pedal = braking_pedal(&tp);
            let mut inp = input(
                dir * pedal,
                0.0,
                [speed; 2],
                [-dir * (engine_drag(&tp, ceiling * k) * k * 0.5 + tp.brake_capacity_n * 0.25); 2],
            );
            inp.sensed = driver_input(speed, inp.reactions.iter().sum(), pedal * dir).sensed;
            let mut state = TransmissionState {
                gear,
                reverse,
                park: false,
                clutch: ClutchState::In,
                launch: LaunchState::Locked,
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

/// A pending reversal uses brakes, never the old ladder's catch against rollback. The player
/// relies on the same control regime whether rest was sampled or skipped between ticks.
/// Removing the no-pedal direction check couples the latched entry on its second tick;
/// the skipped-rest entry also catches a park-only fix.
#[test]
fn pending_reversal_does_not_power_rollback() {
    for (fp, tp) in class_drivetrains() {
        for reverse in [false, true] {
            let dir = if reverse { -1.0 } else { 1.0 };
            let ladder = if reverse {
                &tp.gears_rev
            } else {
                &tp.gears_fwd
            };
            // Fixture: exceed even maximum clutch pull plus brakes by enough to skip the rest
            // band in one tick. The bound is independent of the solved clutch torque.
            let load = 2.0 * tp.brake_capacity_n * tp.brake_static_factor
                + tp.clutch_capacity * ladder[0] / tp.sprocket_radius * 0.5
                + 4.0 * park_engage_speed() * fp.inertia * (1.0 / fixed_step_s());
            // Fixture: the same overload from latched and skipped-rest entries, plus the
            // modest overload from a moving reversal that can otherwise cycle through rest.
            for (entry, load) in [
                (0.0, load),
                (2.0 * park_engage_speed(), load),
                (2.0, 1.5 * tp.brake_capacity_n * tp.brake_static_factor),
            ] {
                let mut state = TransmissionState {
                    reverse,
                    reversal_hold_ticks: (driver_actuation().reversal_hold_s.value
                        * (1.0 / fixed_step_s()))
                    .ceil() as u8,
                    ..fresh(&tp)
                };
                let mut speeds = [dir * entry; 2];
                for tick in 0..(1.0 / fixed_step_s()) as usize * 5 {
                    let mut inp = input(-dir, 0.0, speeds, [dir * load; 2]);
                    inp.sensed = driver_input(speeds[0], dir * 2.0 * load, -dir).sensed;
                    let report = step(&fp, &tp, &mut state, &inp);
                    if speeds[0] * dir <= -park_engage_speed() {
                        assert!(
                            !report.driving,
                            "old ladder powered rollback: entry={entry} tick={tick}"
                        );
                        assert!(
                            report.next_speeds.iter().all(|v| v * dir < 0.0),
                            "an over-capacity slide cannot return to rest"
                        );
                    }
                    if tick == 0 && entry == 2.0 * park_engage_speed() {
                        assert!(
                            report
                                .next_speeds
                                .iter()
                                .all(|v| v * dir <= -park_engage_speed()),
                            "fixture must cross zero without sampling the rest band"
                        );
                        assert!(!state.park);
                    }
                    assert_eq!(
                        state.reverse, reverse,
                        "sliding cannot complete a direction cut"
                    );
                    assert!(!report.shifting);
                    speeds = report.next_speeds;
                }
                assert!(speeds.iter().all(|v| v * dir < -park_engage_speed()));
                assert!(state.reversal_hold_ticks > 0);
                println!(
                    "MEASURED pending reversal gears={} reverse={reverse} entry={entry} final={} park={} clutch={:?}",
                    tp.gears_fwd.len(),
                    speeds[0],
                    state.park,
                    state.clutch
                );
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
        launch: LaunchState::Locked,
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
            inp.supply.engine_in_action = false;
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
            launch: LaunchState::Locked,
            omega_e: tp.engine.crank_limit_rpm * RPM_TO_RAD,
            ..fresh(&tp)
        };
        let mut inp = input(0.0, 0.0, [ceiling; 2], [-grade; 2]);
        inp.supply.task_rate = 0.0;
        inp.supply.engine_in_action = false;
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

/// Admission follows algebraic domains and representability, not a vehicle class's scale.
/// Fixture extremes must retain both representable arithmetic and resolvable conditioning.
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
        / ((1.0 / fixed_step_s()) * RPM_TO_RAD)
        * 1.001;
    a.clutch_capacity_nm = 100000.0;
    a.belt_inertia = 0.5;
    a.shift_secs = 3.5;
    assert!(
        TransmissionParams::from_authoring(&a)
            .unwrap_err()
            .to_string()
            .contains("conditioning")
    );
    a.belt_inertia = 8_000.0; // Fixture: separates the independent steering and stop normals.
    assert!(TransmissionParams::from_authoring(&a).is_ok());
    a.engine_inertia_kgm2 = 0.01;
    a.clutch_capacity_nm = 50.0;
    assert!(TransmissionParams::from_authoring(&a).is_err());
    a.engine_inertia_kgm2 = (200000.0 / engine_control().idle_catch_droop_rpm.value
        + 200000.0 * a.drag_fraction * (1.0 - motoring_drag_base_share()) / 32500.0)
        / ((1.0 / fixed_step_s()) * RPM_TO_RAD)
        * 1.001;
    a.shift_secs = (f32::from(u8::MAX) + 1.0) / (1.0 / fixed_step_s());
    assert!(TransmissionParams::from_authoring(&a).is_err());
    // A cut that rounds to no tick would change gear with the clutch in, unfelt by the ledger.
    a.shift_secs = 0.4 / (1.0 / fixed_step_s());
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
                .get(crate::terrain_surface::surface().base.ground.as_str())
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
    a.engine_inertia_kgm2 = (raw_slope + cut_slope) / (2.0 * (1.0 / fixed_step_s()) * RPM_TO_RAD);
    assert!(TransmissionParams::from_authoring(&a).is_err());
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

/// The launch target follows the interior maximum of raw torque times fuel cut, not a knot
/// or a clamp to governed rpm. Fixture: x rpm past governed, torque `w + 9x` N m across the fuel
/// ramp of width `w` (the base of `w` keeps the rise at 9 N m/rpm at any width), so the pedal's
/// torque `(w + 9x)(1 − x/w)` peaks inside the ramp.
#[test]
fn launch_peak_can_lie_inside_the_fuel_cut_ramp() {
    let w = engine_control().governor_cut_rpm.value;
    let governed = 1000.0;
    let end = governed + 2.0 * w;
    let curve = [
        (800.0, w),
        (governed, w),
        (governed + w, 10.0 * w),
        (end, 10.0 * w),
    ];
    let mut a = tiger_authoring();
    a.governed_rpm = governed;
    a.crank_limit_rpm = end;
    a.torque_nm = &curve;
    let tp = TransmissionParams::from_authoring(&a).unwrap();
    // DERIVED: d/dx (w + 9x)(1 − x/w) = 8 − 18x/w vanishes at x = 4w/9.
    let expected = governed + 4.0 * w / 9.0;
    assert!((tp.peak_torque_rpm - expected).abs() < 0.01); // Resolution: f32 rpm interpolation.
    assert!(tp.torque_at(tp.peak_torque_rpm) > tp.torque_at(a.governed_rpm));
}

/// Admission must include the steep idle catch, independently of the high-rpm fuel ramp.
/// Fixture: a compact sheet at low inertia; operators must get the control widths in its refusal.
#[test]
fn construction_refuses_an_unresolved_idle_catch() {
    let spec = super::super::drive_fixture::compact_spec();
    let tr = &spec.track.powertrain.transmission;
    let mut params = tr.params(0.3, 1.0, 3000.0, spec.mass).unwrap();
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
    let minimum = (idle_slope + drag_slope) / ((1.0 / fixed_step_s()) * RPM_TO_RAD);
    println!(
        "DERIVED compact inertia minimum={minimum:.8} without_idle={:.8} without_drag={:.8}",
        (cut_slope + loss_cut_slope + drag_slope) / ((1.0 / fixed_step_s()) * RPM_TO_RAD),
        idle_slope / ((1.0 / fixed_step_s()) * RPM_TO_RAD)
    );
    for without in [cut_slope + loss_cut_slope + drag_slope, idle_slope] {
        // Midpoints isolate idle_catch and viscous drag independently of every other term.
        params.engine_inertia = (minimum + without / ((1.0 / fixed_step_s()) * RPM_TO_RAD)) * 0.5;
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
    let without = (net_slope + drag_slope) / ((1.0 / fixed_step_s()) * RPM_TO_RAD);
    let with = (net_slope + loss_cut_slope + drag_slope) / ((1.0 / fixed_step_s()) * RPM_TO_RAD);
    println!("DERIVED fuel-cut fixture inertia minimum={with:.8} without_loss_cut={without:.8}");
    params.engine_inertia = (with + without) * 0.5;
    assert!(
        params.check_derived(3000.0).is_err(),
        "loss cut must participate in admission"
    );
    params.engine_inertia = with * 1.001;
    params.check_derived(3000.0).unwrap();
}

/// Fixture: mean/side-clutch constructions, both ladders, subdynamic/static/overload loads,
/// forward/released/opposing requests, steering, entry windows and motion signs. Players rely on
/// fixed intent never selecting alternating active couplings or releasing a passive latch.
/// Gear selection is isolated by an absent ledger; a chosen gear and its crank still integrate.
#[test]
fn fixed_intent_has_no_order_flips_after_one_transition_across_constructions() {
    let mut count = 0;
    for (fp, base) in class_drivetrains() {
        for held_mean in [true, false] {
            let mut tp = base.clone();
            tp.couplings.retain(|c| *c != Coupling::Stop(Output::Mean));
            tp.mean_hold_capacity_n = None;
            if held_mean {
                if !tp.couplings.contains(&Coupling::Steering) {
                    continue;
                }
                tp.couplings.push(Coupling::Stop(Output::Mean));
                tp.mean_hold_capacity_n = Some(2.0 * tp.brake_capacity_n); // Fixture rating.
            }
            for (launch, staffed, latched, slopes) in [
                (LaunchState::Locked, true, false, [0.0, 0.0]),
                (LaunchState::Unchosen, true, false, [80.0, 24_000.0]),
                (LaunchState::TakingUp, true, true, [1200.0, 800_000.0]),
                (LaunchState::Locked, false, true, [800_000.0, 83.0]),
            ] {
                // Fixture: launch/staff/latch/contact states, not authored construction facts.
                for reverse in [false, true] {
                    let dir = if reverse { -1.0 } else { 1.0 };
                    for load_share in [
                        0.5,
                        (1.0 + tp.brake_static_factor) * 0.5,
                        2.0 * tp.brake_static_factor,
                    ] {
                        for command in [0.0, dir, -dir] {
                            for steer in [
                                0.0,
                                (super::super::drive::drive_feel().wide_on.0
                                    + super::super::drive::drive_feel().tight_on.0)
                                    * 0.5,
                                1.0,
                            ] {
                                for window_share in [0.5, 2.0] {
                                    for motion in [dir, -dir] {
                                        count += 1;
                                        let mean = motion
                                            * window_share
                                            * driver_actuation().neutral_speed_m_s.value;
                                        let mut speeds = [mean * 1.1, mean * 0.9];
                                        let mut st = TransmissionState {
                                            reverse,
                                            launch,
                                            mean_hold: latched && held_mean,
                                            ..fresh(&tp)
                                        };
                                        let mut previous = None;
                                        let mut changes = 0;
                                        let mut phase = None;
                                        let mut direction_cuts = 0;
                                        for _ in 0..320 {
                                            // Fixture: five seconds at the fixture clock.
                                            let mut inp = input(
                                                command,
                                                steer,
                                                speeds,
                                                [-motion * load_share * tp.brake_capacity_n; 2],
                                            );
                                            inp.supply.control_authority = staffed;
                                            inp.reaction_slopes = slopes;
                                            // Fixture: asymmetric loads preserve the total demand.
                                            inp.reactions[0] *= 0.6;
                                            inp.reactions[1] *= 1.4;
                                            // No ledger: do not manufacture shifting from a synthetic hull reading.
                                            inp.sensed = None;
                                            let mut preview = st;
                                            let orders =
                                                driver(&construction(&fp), &tp, &mut preview, &inp);
                                            if st.mean_hold
                                                && !preview.mean_hold
                                                && inp.speeds[0] + inp.speeds[1] == 0.0
                                            {
                                                assert_eq!(
                                                    inp.speeds, [0.0; 2],
                                                    "an exit preserving static mean support starts at exact belt rest"
                                                );
                                            }
                                            if staffed
                                                && st.mean_hold
                                                && inp.speeds[0] + inp.speeds[1] != 0.0
                                            {
                                                if command != 0.0 {
                                                    assert!(
                                                        !orders.mean_hold,
                                                        "a breached hold cannot suppress either ladder request"
                                                    );
                                                    if command * dir > 0.0 && !orders.shifting {
                                                        assert!(
                                                            orders.propulsive > 0.0,
                                                            "throttle regains propulsion"
                                                        );
                                                    } else if command * dir < 0.0 {
                                                        assert!(
                                                            orders.service,
                                                            "reverse regains its stopping authority"
                                                        );
                                                    }
                                                } else if steer != 0.0 {
                                                    assert_eq!(
                                                        orders.input.steer, steer,
                                                        "steering retains authority over a sliding mean"
                                                    );
                                                }
                                            }
                                            let signature = (
                                                orders.clutch_open,
                                                preview.park,
                                                orders.mean_hold,
                                                orders.service,
                                                orders.one_way,
                                            );
                                            let next_phase = (preview.reverse, orders.shifting);
                                            if phase != Some(next_phase) {
                                                changes = 0;
                                                previous = None;
                                                phase = Some(next_phase);
                                            }
                                            if previous.is_some_and(|prior| prior != signature) {
                                                changes += 1;
                                            }
                                            previous = Some(signature);
                                            let prior_reverse = st.reverse;
                                            speeds = step(&fp, &tp, &mut st, &inp).next_speeds;
                                            direction_cuts +=
                                                usize::from(st.reverse != prior_reverse);
                                            assert!(
                                                direction_cuts <= 1,
                                                "fixed intent permits one direction cut"
                                            );
                                            assert!(
                                                changes <= 1,
                                                "one settling transition per fixed Driver phase"
                                            );
                                        }
                                        assert!(
                                            changes <= 1,
                                            "fixed intent chattered: mean_held={held_mean} reverse={reverse} load={load_share} command={command} steer={steer} entry={mean}: {changes}"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(
        count, 6048,
        "the enumerated issue classes must remain complete"
    );
}

/// Fixture: every geared-steering construction with two side brakes linked to its mean hold.
fn mean_hold_drivetrains() -> impl Iterator<Item = (FixtureConstruction, TransmissionParams)> {
    class_drivetrains()
        .into_iter()
        .filter(|(_, tp)| tp.couplings.contains(&Coupling::Steering))
        .map(|(fp, mut tp)| {
            if !tp.has_mean_hold() {
                tp.couplings.push(Coupling::Stop(Output::Mean));
                tp.mean_hold_capacity_n = Some(2.0 * tp.brake_capacity_n); // Fixture linkage.
            }
            (fp, tp)
        })
}

/// Releasing a pivot must transfer a supported common-mode load without losing static grip.
/// Players on a slope rely on retaining the held mean until the straight tie has stopped both
/// belts; a speed-band handover or dropping the hold on steer release fails this witness.
#[test]
fn pivot_release_hands_static_load_to_side_stops_from_exact_rest() {
    for (fp, tp) in mean_hold_drivetrains() {
        for reverse in [false, true] {
            for direction in [-1.0, 1.0] {
                let load = direction * tp.brake_capacity_n * (1.0 + tp.brake_static_factor) * 0.5;
                for brake in [false, true] {
                    let mut st = TransmissionState {
                        reverse,
                        mean_hold: true,
                        ..fresh(&tp)
                    };
                    // Fixture: opposite belt motion at a resting mean, as a pivot's steer ramp ends.
                    let mut speeds = [0.018, -0.018];
                    let mut transferred = false;
                    for _ in 0..16 {
                        let start = speeds;
                        let mut inp = input(0.0, if brake { 1.0 } else { 0.0 }, speeds, [load; 2]);
                        if brake {
                            inp.intent = DriveIntent::Brake;
                        }
                        let report = step(&fp, &tp, &mut st, &inp);
                        speeds = report.next_speeds;
                        assert_eq!(
                            speeds[0] + speeds[1],
                            0.0,
                            "handover cannot drop the supported mean"
                        );
                        if st.park {
                            assert_eq!(
                                start, [0.0; 2],
                                "replacement stops latch only from exact rest"
                            );
                            assert!(!st.mean_hold);
                            assert_eq!(speeds, [0.0; 2]);
                            transferred = true;
                        } else {
                            assert!(st.mean_hold);
                        }
                    }
                    assert!(
                        transferred,
                        "straight steering must arrest the remaining differential motion"
                    );
                }
            }
        }
    }
}

/// A passive drivetrain may transfer belt energy into its crank, but cannot create total
/// kinetic energy. Players rely on steering and the clutch sharing one crank budget.
/// Billing a solved steering impulse to the belts alone would break this energy bound.
#[test]
fn no_fuel_or_external_work_cannot_raise_total_kinetic_energy() {
    for (fp, tp) in class_drivetrains() {
        for steer in [-1.0, 0.0, 1.0] {
            for entry in [[0.0; 2], [2.0; 2], [1.0, -1.0]] {
                // Fixture: rest, drive, and pivot energy.
                let mut st = TransmissionState {
                    omega_e: tp.engine.governed_rpm * RPM_TO_RAD,
                    launch: LaunchState::Locked,
                    ..fresh(&tp)
                };
                let mut speeds = entry;
                for _ in 0..128 {
                    let energy = |omega: f32, v: [f32; 2]| {
                        0.5 * f64::from(tp.engine_inertia) * f64::from(omega).powi(2)
                            + 0.5
                                * f64::from(fp.inertia)
                                * v.into_iter().map(|x| f64::from(x).powi(2)).sum::<f64>()
                    };
                    let before = energy(st.omega_e, speeds);
                    let mut inp = input(0.0, steer, speeds, [0.0; 2]);
                    // The game's damage model: no served combustion or Driver catch, construction drag remains.
                    inp.supply.task_rate = 1.0;
                    inp.supply.engine_in_action = false;
                    speeds = step(&fp, &tp, &mut st, &inp).next_speeds;
                    let after = energy(st.omega_e, speeds);
                    // Resolution: cast of the f64 block solution into replicated f32 speeds.
                    assert!(
                        after <= before * (1.0 + 8.0 * f64::from(f32::EPSILON)) + 1e-8,
                        "passive energy grew {before} -> {after}"
                    );
                }
            }
        }
    }
}

/// Locked clutch and steering rows reproduce construction radii across both ladders. The HUD
/// and Driver rely on one crank ratio per detent; the table is allowed only its declared rounding.
#[test]
fn locked_rows_reproduce_authored_radii_within_the_rounding_bound() {
    for (fp, tp) in class_drivetrains() {
        if !tp.couplings.contains(&Coupling::Steering) {
            continue;
        }
        for reverse in [false, true] {
            for gear in 1..=if reverse {
                tp.gears_rev.len()
            } else {
                tp.gears_fwd.len()
            } as u8
            {
                for (steer, index) in [
                    (1.0, 0),
                    (
                        (super::super::drive::drive_feel().wide_on.0
                            + super::super::drive::drive_feel().tight_on.0)
                            * 0.5,
                        1,
                    ),
                ] {
                    let dir = if reverse { -1.0 } else { 1.0 };
                    let omega = tp.engine.governed_rpm * RPM_TO_RAD;
                    let mean = dir * omega / (tp.ratio(reverse, gear) / tp.sprocket_radius);
                    let c = if index == 0 {
                        tp.steer_crank_ratio.0
                    } else {
                        tp.steer_crank_ratio.1
                    };
                    let mut st = TransmissionState {
                        gear,
                        reverse,
                        omega_e: omega,
                        launch: LaunchState::Locked,
                        clutch: ClutchState::In,
                        ..fresh(&tp)
                    };
                    let inp = input(dir, steer, [mean + c * omega, mean - c * omega], [0.0; 2]);
                    let rep = step(&fp, &tp, &mut st, &inp);
                    assert!(rep.locked, "unloaded synchronous clutch must hold");
                    let [left, right] = rep.next_speeds;
                    let radius_over_half_tread = ((left + right) / (left - right)).abs();
                    let radius = radius_over_half_tread * tp.half_tread_m;
                    let pair = tp.steering_radii(reverse, gear);
                    let authored = if index == 0 { pair.0 } else { pair.1 };
                    // Resolution: subtraction of near-equal f32 belt speeds at wide radii.
                    let allowance =
                        authored * (tp.steering_radius_rounding_share + 64.0 * f32::EPSILON);
                    assert!(
                        (radius - authored).abs() <= allowance,
                        "radius {radius} vs table {authored}"
                    );
                }
            }
        }
    }
}

/// Fixture: two side clutches, each with its belt's brake, without a steering tie. Future
/// constructions rely on the production assembler, rather than a special analytic drivetrain.
#[test]
fn clutch_and_brake_construction_drives_turns_and_stops_through_step() {
    let (fp, tp) = side_clutch_drivetrain();
    let mut st = fresh(&tp);
    let mut speeds = [0.0; 2];
    for _ in 0..128 {
        let mut inp = input(1.0, 0.0, speeds, [0.0; 2]);
        inp.sensed = Some(Sensed::steady(0.0, 0.0, 0.0));
        speeds = step(&fp, &tp, &mut st, &inp).next_speeds;
    }
    assert!(speeds[0] > 0.0 && speeds[1] > 0.0);
    for _ in 0..64 {
        let mut inp = input(1.0, 1.0, speeds, [0.0; 2]);
        inp.sensed = Some(Sensed::steady(0.0, 0.0, 0.0));
        speeds = step(&fp, &tp, &mut st, &inp).next_speeds;
    }
    assert!(
        speeds[0] > speeds[1] && speeds[1] == 0.0,
        "inner clutch releases and its brake holds"
    );
    for _ in 0..128 {
        let mut inp = input(0.0, 0.0, speeds, [0.0; 2]);
        inp.intent = DriveIntent::Brake;
        speeds = step(&fp, &tp, &mut st, &inp).next_speeds;
    }
    assert_eq!(speeds, [0.0; 2]);
}

/// The asset loader must refuse a radius table that contradicts its single crank ratio.
/// Designers rely on a named field and repair, rather than a per-gear steering exception.
#[test]
fn inconsistent_steering_radii_are_refused_at_load() {
    for reverse in [false, true] {
        for wide in [false, true] {
            let mut a = lab_authoring();
            let mut radii = if reverse {
                a.reverse_steer_radii_m
            } else {
                a.steer_radii_m
            }
            .to_vec();
            // Fixture: an error well beyond the declared rounding, without changing the anchor.
            let index = radii.len() - 1;
            if wide {
                radii[index].1 *= 2.0;
            } else {
                radii[index].0 *= 2.0;
            }
            if reverse {
                a.reverse_steer_radii_m = &radii;
            } else {
                a.steer_radii_m = &radii;
            }
            let error = TransmissionParams::from_authoring(&a)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("radii[")
                    && error.contains("radius_rounding_share")
                    && error.contains("author radii"),
                "{error}"
            );
        }
    }
}

/// A layout must describe distinct paths and be able to finish its own passive handover.
/// Asset authors get a refusal before play; no missing connection gets a code default.
#[test]
fn invalid_coupling_construction_is_refused_before_play() {
    let invalid: &[&[Coupling]] = &[
        &[],
        &[Coupling::Clutch(Output::Left)],
        &[
            Coupling::Clutch(Output::Mean),
            Coupling::Clutch(Output::Mean),
        ],
        &[
            Coupling::Clutch(Output::Left),
            Coupling::Clutch(Output::Right),
            Coupling::Stop(Output::Mean),
        ],
    ];
    for couplings in invalid {
        let mut a = lab_authoring();
        a.couplings = couplings;
        let error = TransmissionParams::from_authoring(&a)
            .unwrap_err()
            .to_string();
        assert!(error.contains("transmission.couplings"), "{error}");
    }
}

/// A supported park may hand its load to a pivot hold or a service stop at exact rest.
/// Players rely on the static budget surviving a change of passive stop ownership.
#[test]
fn parked_static_load_survives_pivot_and_service_handover() {
    for (fp, tp) in mean_hold_drivetrains() {
        for sign in [-1.0, 1.0] {
            let load = sign * tp.brake_capacity_n * (1.0 + tp.brake_static_factor) * 0.5;
            for pivot in [false, true] {
                let mut state = TransmissionState {
                    park: true,
                    ..fresh(&tp)
                };
                let mut inp = input(0.0, 1.0, [0.0; 2], [load; 2]);
                if !pivot {
                    inp.intent = DriveIntent::Brake;
                }
                let report = step(&fp, &tp, &mut state, &inp);
                assert_eq!(report.next_speeds[0] + report.next_speeds[1], 0.0);
                if !pivot {
                    assert_eq!(report.next_speeds, [0.0; 2]);
                }
                assert_eq!(state.mean_hold, pivot);
            }
        }
    }
}

/// The guards constrain the sum of every crank reaction, including the steering member.
/// The engine model relies on this instead of an energy-adding floor clamp after the solve.
#[test]
fn crank_guards_bound_combined_clutch_and_steering_reactions() {
    for (fp, tp) in class_drivetrains() {
        for pending in [false, true] {
            for steer in [-1.0, 0.0, 1.0] {
                for motion in [-1.0, 0.0, 1.0] {
                    let omega = tp.engine.governed_rpm * RPM_TO_RAD;
                    let speed = motion * omega * tp.sprocket_radius / tp.gears_fwd[0];
                    let mut state = TransmissionState {
                        omega_e: omega,
                        launch: if pending {
                            LaunchState::TakingUp
                        } else {
                            LaunchState::Locked
                        },
                        ..fresh(&tp)
                    };
                    let mut inp = input(1.0, steer, [speed; 2], [tp.brake_capacity_n; 2]);
                    inp.sensed = Some(Sensed::steady(speed, 0.0, 0.0)); // Fixture: no paid gear decision.
                    let [fuel, catch, drag] = engine_torques(&tp, inp.supply, omega, 1.0);
                    let free = omega + (fuel + catch - drag) * inp.dt / tp.engine_inertia;
                    let floor = if pending {
                        tp.launch_omega(1.0)
                    } else {
                        (tp.engine.idle_rpm - engine_control().idle_guard_band_rpm.value)
                            * RPM_TO_RAD
                    }
                    .min(free);
                    let over =
                        ((tp.engine.crank_limit_rpm + overrev_margin_rpm()) * RPM_TO_RAD).max(free);
                    step(&fp, &tp, &mut state, &inp);
                    // Resolution: f64 block arithmetic cast to replicated f32 crank speed.
                    let rounding = 8.0 * f32::EPSILON * over;
                    assert!(
                        state.omega_e >= floor - rounding && state.omega_e <= over + rounding,
                        "combined reactions crossed guard [{floor}, {over}]: {}",
                        state.omega_e
                    );
                }
            }
        }
    }
}

/// A paid shift matches the signed shaft along its ladder. Reverse players rely on the same
/// crank response; using the world's signed mean makes a reverse blip target idle instead.
#[test]
fn paid_rev_matching_uses_the_signed_shaft_on_both_ladders() {
    for (fp, tp) in class_drivetrains() {
        let mut endings = Vec::new();
        for reverse in [false, true] {
            let dir = if reverse { -1.0 } else { 1.0 };
            let target = (tp.engine.idle_rpm + tp.engine.governed_rpm) * 0.5 * RPM_TO_RAD;
            let speed = dir * target * tp.sprocket_radius / tp.ratio(reverse, 1);
            let mut state = TransmissionState {
                reverse,
                shift_ticks: 1,
                ..fresh(&tp)
            };
            let report = step(&fp, &tp, &mut state, &input(0.0, 0.0, [speed; 2], [0.0; 2]));
            assert!(report.shifting && !report.driving);
            assert!(
                state.omega_e > tp.engine.idle_rpm * RPM_TO_RAD,
                "a live rack must blip toward the higher shaft speed"
            );
            endings.push(state.omega_e);
        }
        // Resolution: forming the same target through distinct f32 ladder reductions.
        assert!((endings[0] - endings[1]).abs() <= 8.0 * f32::EPSILON * endings[0].abs());
    }
}

/// Closing a paid interruption must retain the ratio that interruption selected. The Driver
/// relies on a free initial choice never repricing a moving cut's selected gear.
#[test]
fn a_paid_cut_closes_onto_its_selected_ratio() {
    for (fp, tp) in class_drivetrains() {
        let gear = 2; // Fixture: an interior ratio with higher safe gears available.
        let omega = (tp.engine.idle_rpm + tp.engine.governed_rpm) * 0.5 * RPM_TO_RAD;
        let speed = omega * tp.sprocket_radius / tp.ratio(false, gear);
        let mut state = TransmissionState {
            gear,
            omega_e: omega,
            shift_ticks: 1,
            launch: LaunchState::TakingUp,
            ..fresh(&tp)
        };
        let mut inp = driver_input(speed, 0.0, 1.0);
        let cut = step(&fp, &tp, &mut state, &inp);
        assert!(cut.shifting);
        inp.speeds = cut.next_speeds;
        let closed = step(&fp, &tp, &mut state, &inp);
        assert!(
            !closed.shifting && state.gear == gear,
            "the open result of the last cut tick is not another free selector order"
        );
    }
}

/// Supported driver requests obey the same passive handover as Brake.
/// Players rely on the existing static support surviving until side park starts at exact rest.
/// Rest persists when the FULL constrained demand (including differential changes) fits capacity.
#[test]
fn supported_driver_handover_keeps_support_through_an_unequal_ground_steer_ramp() {
    for (fp, tp) in mean_hold_drivetrains() {
        for launch in [
            LaunchState::Unchosen,
            LaunchState::TakingUp,
            LaunchState::Locked,
        ] {
            for path in [0, 1, 2, 4] {
                // Fixture: release, Brake, opposing request and along-ladder request.
                // Loss of staff and stored-crank discharge are covered by the shipped sleep proof.
                let mut st = TransmissionState {
                    launch,
                    mean_hold: true,
                    omega_e: 0.0, // Fixture: stored crank energy is absent from this passive-support proof.
                    ..fresh(&tp)
                };
                let mut speeds = [0.018, -0.018]; // Fixture pivot entry, mean exactly resting.
                // Fixture: the full initial differential arrest contributes four times the
                // starting common load, while remaining inside every fixture's static rating.
                let total = (800_000.0 - 80.0) * speeds[0] * 0.25;
                let mut largest_coupled_load = 0.0_f32;
                let mut parked = false;
                for tick in 0..128 {
                    let wide = (super::super::drive::drive_feel().wide_on.0
                        + super::super::drive::drive_feel().tight_on.0)
                        * 0.5;
                    let steer = if tick < 8 {
                        1.0
                    } else if tick < 16 {
                        wide
                    } else {
                        0.0
                    };
                    let mut inp = input(0.0, steer, speeds, [0.45 * total, 0.55 * total]);
                    inp.supply.engine_in_action = false; // Fixture: no combustion during passive handover.
                    inp.reaction_slopes = [80.0, 800_000.0]; // Fixture: stiff unequal soil couples differential changes into mean load.
                    if tick >= 16 {
                        match path {
                            0 => {}
                            1 => inp.intent = DriveIntent::Brake,
                            2 => {
                                inp.intent = DriveIntent::Axes(DriveAxes {
                                    throttle: -1.0,
                                    steer: 1.0,
                                });
                                inp.throttle = -1.0;
                                inp.steer = 1.0;
                            }
                            4 => {
                                inp.intent = DriveIntent::Axes(DriveAxes {
                                    throttle: 1.0,
                                    steer: 1.0,
                                });
                                inp.throttle = 1.0;
                                inp.steer = 1.0;
                            }
                            _ => unreachable!("the fixture enumerates four driver handover paths"),
                        }
                    }
                    let start = speeds;
                    let before_hold = st.mean_hold;
                    let report = step(&fp, &tp, &mut st, &inp);
                    speeds = report.next_speeds;
                    let delta = ((start[0] - start[1]) - (speeds[0] - speeds[1])) * 0.5;
                    largest_coupled_load = largest_coupled_load
                        .max(((inp.reaction_slopes[1] - inp.reaction_slopes[0]) * delta).abs());
                    if before_hold && !st.mean_hold {
                        assert_eq!(
                            start, [0.0; 2],
                            "passive replacement must start at exact rest"
                        );
                        if path == 4 {
                            assert!(
                                !st.park,
                                "along-ladder intent takes the one-way launch lever"
                            );
                            assert!(
                                report.next_speeds.iter().all(|v| *v >= 0.0),
                                "the supported static replacement prevents rollback"
                            );
                        } else {
                            assert!(st.park);
                            assert_eq!(report.held, [true; 2], "rest is visible to the sleep gate");
                        }
                        parked = true;
                        break; // An opposing request may drive after its paid reversal, outside this handover.
                    }
                    if path == 4 && st.mean_hold {
                        assert!(
                            !report.pulling && !report.driving,
                            "propulsion waits for its passive handover instead of fighting the held stop"
                        );
                    }
                    assert_eq!(
                        speeds[0] + speeds[1],
                        0.0,
                        "this fixture's full demand remains supported"
                    );
                }
                assert!(
                    largest_coupled_load > total * 0.25,
                    "the coupling term must be substantial against the load"
                );
                assert!(
                    parked,
                    "every passive exit can finish its exact-rest handover"
                );
            }
        }
    }
}

/// With affine contacts, ground work is external work even when its starting force is zero.
/// The production assembly may transfer energy between crank/belts, but cannot create energy
/// beyond endpoint external work (the backward-Euler dissipation inequality).
#[test]
fn affine_ground_energy_account_covers_launch_staff_and_latched_states() {
    for (fp, tp) in class_drivetrains() {
        for launch in [
            LaunchState::Unchosen,
            LaunchState::TakingUp,
            LaunchState::Locked,
        ] {
            for staffed in [false, true] {
                for latched in [false, true] {
                    if latched && !tp.has_mean_hold() {
                        continue;
                    }
                    let mut st = TransmissionState {
                        launch,
                        mean_hold: latched,
                        omega_e: tp.engine.governed_rpm * RPM_TO_RAD,
                        ..fresh(&tp)
                    };
                    let mut speeds = [1.0, -0.5]; // Fixture incompatible crank/mean/difference entry.
                    for tick in 0..48 {
                        let wide = (super::super::drive::drive_feel().wide_on.0
                            + super::super::drive::drive_feel().tight_on.0)
                            * 0.5;
                        let steer = if tick < 16 {
                            1.0
                        } else if tick < 32 {
                            wide
                        } else {
                            0.0
                        };
                        let mut inp = input(0.0, steer, speeds, [1000.0, -700.0]);
                        inp.reaction_slopes = [80.0, 800_000.0]; // Fixture unequal affine contacts.
                        inp.supply.control_authority = staffed;
                        inp.supply.engine_in_action = false; // The game's damage model: no combustion or catch.
                        let omega = st.omega_e;
                        let energy = |w: f32, v: [f32; 2]| {
                            0.5 * f64::from(tp.engine_inertia) * f64::from(w).powi(2)
                                + 0.5
                                    * f64::from(fp.inertia)
                                    * v.into_iter().map(|v| f64::from(v).powi(2)).sum::<f64>()
                        };
                        let before = energy(omega, speeds);
                        let report = step(&fp, &tp, &mut st, &inp);
                        let next = report.next_speeds;
                        let crank_work =
                            -f64::from(bounded_drag(&tp, omega, inp.dt, engine_drag(&tp, omega)))
                                * f64::from(st.omega_e)
                                * f64::from(inp.dt);
                        let ground_work: f64 = (0..2)
                            .map(|i| {
                                -f64::from(
                                    inp.reactions[i]
                                        + inp.reaction_slopes[i] * (next[i] - speeds[i]),
                                ) * f64::from(next[i])
                                    * f64::from(inp.dt)
                            })
                            .sum();
                        let after = energy(st.omega_e, next);
                        // Resolution: f32 state publication and affine-reaction cancellation.
                        let allowance = 64.0
                            * f64::from(f32::EPSILON)
                            * (1.0 + before + after + ground_work.abs());
                        assert!(
                            after <= before + crank_work + ground_work + allowance,
                            "unbilled energy: {before} -> {after}, work={}",
                            crank_work + ground_work
                        );
                        speeds = next;
                    }
                }
            }
        }
    }
}

/// A crank-limited launch shares the reactions actually used by clutch and steering.
/// Raising an unused steering rating cannot steal propulsion; an advance reservation does.
#[test]
fn unused_steering_rating_cannot_reduce_a_powered_launch() {
    let (fp, base) = (lab_fp(), lab_tp());
    let omega = base.launch_omega(1.0);
    let d = base.steer_crank_ratio.0 * omega;
    let mean = 0.5 * omega * base.sprocket_radius / base.gears_fwd[0];
    let mut outcomes = Vec::new();
    for share in [2.0, 4.0] {
        // Fixture: two ratings above the solved steering demand.
        let mut tp = base.clone();
        tp.steer_capacity_n *= share;
        let mut st = TransmissionState {
            omega_e: omega,
            launch: LaunchState::TakingUp,
            ..fresh(&tp)
        };
        let inp = input(1.0, 1.0, [mean + d, mean - d], [0.0; 2]);
        let controls = driver(&construction(&fp), &tp, &mut st, &inp);
        let [fuel, catch, drag] = engine_torques(&tp, inp.supply, omega, 1.0);
        let (report, _, steering) = advance_rows(
            &construction(&fp),
            &tp,
            &mut st,
            &controls,
            fuel + catch - bounded_drag(&tp, omega, inp.dt, drag),
        );
        assert!(
            steering.abs() < 2.0 * tp.steer_capacity_n,
            "the member rating must be unused"
        );
        assert!(report.driving, "the witness must deliver clutch propulsion");
        outcomes.push(report.forces.iter().sum::<f32>());
    }
    // Resolution: f64 elimination followed by f32 publication, scaled by delivered force.
    assert!((outcomes[0] - outcomes[1]).abs() <= 8.0 * f32::EPSILON * outcomes[0].abs());
    assert!(outcomes[0] > 0.0, "the witness must deliver propulsion");
}

/// Required ratings belong to the declared device, never a code default or another brake.
#[test]
fn a_mean_stop_requires_its_own_positive_rating_and_no_other_construction_consumes_it() {
    let mut a = tiger_authoring();
    a.mean_hold_capacity_n = None;
    assert!(
        TransmissionParams::from_authoring(&a)
            .unwrap_err()
            .to_string()
            .contains("mean_hold_brake_count")
    );
    for rating in [0.0, -1.0, f32::INFINITY, f32::NAN] {
        // Fixture malformed asset inputs.
        a.mean_hold_capacity_n = Some(rating);
        assert!(TransmissionParams::from_authoring(&a).is_err());
    }
    let mut a = lab_authoring();
    a.mean_hold_capacity_n = Some(192_000.0); // Fixture rating without a device is invalid.
    assert!(
        TransmissionParams::from_authoring(&a)
            .unwrap_err()
            .to_string()
            .contains("mean_hold_brake_count")
    );
}

/// A breached passive hold cannot trap the driver under a load it cannot arrest. Forward,
/// service, steer and reverse orders must reach their constructed actuators immediately.
#[test]
fn an_overloaded_sliding_hold_returns_every_driver_authority() {
    for (fp, tp) in mean_hold_drivetrains() {
        for reverse in [false, true] {
            let dir = if reverse { -1.0 } else { 1.0 };
            for path in 0..4 {
                let mut st = TransmissionState {
                    reverse,
                    mean_hold: true,
                    launch: LaunchState::Locked,
                    ..fresh(&tp)
                };
                let mut inp = input(
                    0.0,
                    0.0,
                    [-dir; 2],
                    [2.0 * tp.mean_hold_capacity_n.unwrap(); 2],
                ); // Fixture: load exceeds the dynamic rating.
                match path {
                    0 | 3 => {
                        inp.throttle = if path == 0 { dir } else { -dir };
                        inp.intent = DriveIntent::Axes(DriveAxes {
                            throttle: inp.throttle,
                            steer: 0.0,
                        });
                    }
                    1 => inp.intent = DriveIntent::Brake,
                    2 => inp.steer = 1.0,
                    _ => unreachable!(),
                }
                let orders = driver(&construction(&fp), &tp, &mut st, &inp);
                match path {
                    0 => assert!(orders.propulsive > 0.0 && !orders.mean_hold),
                    1 | 3 => {
                        assert!(orders.service && !orders.mean_hold);
                        assert_eq!(
                            passive_rows(
                                &tp,
                                &st,
                                &orders,
                                f64::from(tp.brake_capacity_n * inp.dt)
                            )
                            .len(),
                            2
                        );
                    }
                    2 => assert_eq!(orders.input.steer, 1.0),
                    _ => unreachable!(),
                }
            }
        }
    }
}

/// An author can state consistent radii that still yield an unresolved steering/stop basis.
/// Admission must refuse that construction before an operator can reach the solver panic.
#[test]
fn construction_refuses_consistent_but_unresolved_steering_rows() {
    let mut a = lab_authoring();
    let radii: Vec<_> = a
        .steer_radii_m
        .iter()
        .map(|(t, w)| (t * 100_000.0, w * 100_000.0))
        .collect();
    let reverse: Vec<_> = a
        .reverse_steer_radii_m
        .iter()
        .map(|(t, w)| (t * 100_000.0, w * 100_000.0))
        .collect();
    a.steer_radii_m = &radii;
    a.reverse_steer_radii_m = &reverse;
    let error = TransmissionParams::from_authoring(&a)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("conditioning") && error.contains("steering.radii"),
        "{error}"
    );
}
