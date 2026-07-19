use super::*;

/// A lab ForceParams: only the fields the transmission reads matter (inertia, max_speed,
/// slip_saturation, grip_stiffness envelope switch, and the governor's own knobs).
fn lab_fp() -> ForceParams {
    ForceParams {
        thickness: 0.04,
        columns: [(-0.25, 1.0 / 6.0), (0.0, 2.0 / 3.0), (0.25, 1.0 / 6.0)],
        support_stiffness_per_m: 680_000.0,
        support_damping_per_m: 80_000.0,
        engage_depth: 0.02,
        probe_reach: 0.5,
        mu: 0.9,
        lateral_ratio: 0.55,
        slip_saturation: 0.4,
        max_speed: 15.0,
        engine_power: 186_500.0,
        engine_force: 120_000.0,
        governor_gain: 60_000.0,
        inertia: 8_000.0,
        grip_stiffness: forces::grip_stiffness(0.9, 26_500.0 * 9.81),
    }
}

/// A lab transmission: T-34-flavoured plausible tables (the sandbox's config shape).
fn lab_tp() -> TransmissionParams {
    TransmissionParams::from_authoring(&TransmissionAuthoring {
        idle_rpm: 600.0,
        governed_rpm: 1800.0,
        rated_rpm: 1800.0,
        torque_nm: &[
            (600.0, 1650.0),
            (1100.0, 2200.0),
            (1700.0, 1950.0),
            (1800.0, 0.0),
        ],
        forward_speeds_kmh: &[8.0, 12.7, 20.4, 32.6, 52.2],
        reverse_speeds_kmh: &[8.0],
        shift_up_rpm: 1700.0,
        shift_down_rpm: 950.0,
        steer_radii_m: &[
            (3.0, 8.9),
            (4.8, 14.2),
            (7.7, 22.8),
            (12.3, 36.4),
            (19.7, 58.3),
        ],
        steer_capacity_n: 240_000.0,
        recirculation: 0.9,
        brake_capacity_n: 120_000.0,
        brake_static_factor: 1.6,
        drag_fraction: 0.25,
        // Stage B lab crank: same class band as the vehicle authoring (J mid-band,
        // clutch ≈ 1.3 × the 2200 N·m peak).
        engine_inertia_kgm2: 4.0,
        clutch_capacity_nm: 2860.0,
        belt_inertia: 8_000.0,
        shift_secs: 0.31,
        shift_addressing: ShiftAddressing::Sequential,
        sprocket_radius_m: 0.34,
        half_tread_m: 1.25,
    })
    .expect("lab transmission authoring must be valid")
}

/// The shipped Tiger's declared drivetrain, kept local to arithmetic tests so they exercise
/// the same authored curve and speed anchors without reaching through the ECS/spec adapter.
fn tiger_tp() -> TransmissionParams {
    TransmissionParams::from_authoring(&TransmissionAuthoring {
        idle_rpm: 600.0,
        governed_rpm: 2500.0,
        rated_rpm: 3000.0,
        torque_nm: &[
            (800.0, 1300.0),
            (2100.0, 1850.0),
            (2500.0, 1686.0),
            (3000.0, 1639.0),
        ],
        forward_speeds_kmh: &[2.8, 4.3, 6.2, 9.2, 14.1, 20.9, 30.5, 45.4],
        reverse_speeds_kmh: &[2.8, 4.3, 6.2, 9.2],
        shift_up_rpm: 2300.0,
        shift_down_rpm: 1400.0,
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
        steer_capacity_n: 250_000.0,
        recirculation: 0.9,
        brake_capacity_n: 96_000.0,
        brake_static_factor: 1.5,
        drag_fraction: 0.25,
        engine_inertia_kgm2: 4.0,
        clutch_capacity_nm: 2400.0,
        belt_inertia: 16_000.0,
        shift_secs: 0.31,
        shift_addressing: ShiftAddressing::Direct,
        sprocket_radius_m: 19.0 * 0.130 / std::f32::consts::TAU,
        half_tread_m: 1.4904,
    })
    .expect("Tiger transmission authoring must be valid")
}

fn input(throttle: f32, steer: f32, speeds: [f32; 2], reactions: [f32; 2]) -> TransmissionInput {
    TransmissionInput {
        throttle,
        steer,
        side_commands: [
            (throttle + steer).clamp(-1.0, 1.0),
            (throttle - steer).clamp(-1.0, 1.0),
        ],
        speeds,
        reactions,
        dt: 1.0 / 64.0,
    }
}

fn fresh(tp: &TransmissionParams) -> TransmissionState {
    TransmissionState::from_spec(tp)
}

fn assert_report_bits_eq(actual: &TransmissionReport, expected: &TransmissionReport) {
    assert_eq!(
        actual.next_speeds.map(f32::to_bits),
        expected.next_speeds.map(f32::to_bits)
    );
    assert_eq!(
        actual.forces.map(f32::to_bits),
        expected.forces.map(f32::to_bits)
    );
    assert_eq!(actual.rpm.to_bits(), expected.rpm.to_bits());
    assert_eq!(actual.gear, expected.gear);
    assert_eq!(actual.reverse, expected.reverse);
    assert_eq!(actual.steer_step, expected.steer_step);
    assert_eq!(actual.shifting, expected.shifting);
    assert_eq!(actual.power_scale.to_bits(), expected.power_scale.to_bits());
    assert_eq!(
        actual.power_available.to_bits(),
        expected.power_available.to_bits()
    );
}

fn assert_first_tick_matches_old_lazy_init(
    mode: TransmissionMode,
    fp: &ForceParams,
    tp: &TransmissionParams,
    inp: &TransmissionInput,
) {
    let mut old_lazy = TransmissionState::for_governor();
    old_lazy.omega_e = tp.engine.idle_rpm * RPM_TO_RAD;
    let mut explicit = TransmissionState::from_spec(tp);

    let old_report = step(mode, fp, Some(tp), &mut old_lazy, inp);
    let explicit_report = step(mode, fp, Some(tp), &mut explicit, inp);

    assert_report_bits_eq(&explicit_report, &old_report);
    assert_eq!(explicit, old_lazy);
    assert_eq!(explicit.omega_e.to_bits(), old_lazy.omega_e.to_bits());
    assert_eq!(explicit.demand_n.to_bits(), old_lazy.demand_n.to_bits());
}

#[test]
fn from_spec_preserves_old_lazy_first_tick_bits() {
    let fp = lab_fp();
    let tp = lab_tp();
    assert_first_tick_matches_old_lazy_init(
        TransmissionMode::Hybrid,
        &fp,
        &tp,
        &input(1.0, 0.0, [0.0; 2], [0.0; 2]),
    );
    assert_first_tick_matches_old_lazy_init(
        TransmissionMode::FixedRadii,
        &fp,
        &tp,
        &input(1.0, 0.8, [0.0; 2], [80_000.0; 2]),
    );
}

#[test]
fn rev14_transmission_state_inventory_tripwire() {
    const REPLICATE_EXACT_FIELDS: usize = 16;
    const DERIVE_FIELDS: usize = 0;
    const LOCAL_VIEW_FIELDS: usize = 0;

    // Adding a field? Classify it in transmission-design.md's authoritative REV-14 inventory,
    // then extend this exhaustive destructure and the classified name list. Do not add `..`.
    let TransmissionState {
        gear,
        shift_ticks,
        steer_step,
        reverse,
        park,
        last_shift_dir,
        dwell_ticks,
        omega_e,
        clutch_out,
        demand_n,
        demand_initialized,
        grade_confirm_ticks,
        grade_target,
        scheduler,
        hill_hold,
        hold_reengage_ticks,
    } = fresh(&lab_tp());
    let classified_fields = [
        "gear",
        "shift_ticks",
        "steer_step",
        "reverse",
        "park",
        "last_shift_dir",
        "dwell_ticks",
        "omega_e",
        "clutch_out",
        "demand_n",
        "demand_initialized",
        "grade_confirm_ticks",
        "grade_target",
        "scheduler",
        "hill_hold",
        "hold_reengage_ticks",
    ];
    let _ = (
        gear,
        shift_ticks,
        steer_step,
        reverse,
        park,
        last_shift_dir,
        dwell_ticks,
        omega_e,
        clutch_out,
        demand_n,
        demand_initialized,
        grade_confirm_ticks,
        grade_target,
        scheduler,
        hill_hold,
        hold_reengage_ticks,
    );

    assert_eq!(
        classified_fields.len(),
        REPLICATE_EXACT_FIELDS + DERIVE_FIELDS + LOCAL_VIEW_FIELDS
    );
}

/// Stage C reserve arithmetic at the slope investigation's reconstructed operating point.
/// At the belt speed that puts Tiger F4 at 980 rpm DERIVED, its authored curve gives about
/// 169 kN DERIVED total sprocket force (the investigation's 165 kN DERIVED rounding), below the DERIVED 20°
/// grade demand `57_000 * 9.81 * sin(20°) = 191.2 kN`. F3 at the same speed has enough
/// reserve to clear the DERIVED 10% + absolute margin.
#[test]
fn reserve_uses_authored_curve_and_traction_cap() {
    let tp = tiger_tp();
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    let f4 = tp.gears_fwd[3];
    let shaft = 980.0 * RPM_TO_RAD * tp.sprocket_radius / f4;
    let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();

    let force_f4 = available_force_in_gear(&tp, &fp, shaft, f4);
    let force_f3 = available_force_in_gear(&tp, &fp, shaft, tp.gears_fwd[2]);
    let margin = reserve_margin(demand);

    assert!(
        (165_000.0..=172_000.0).contains(&force_f4),
        "F4 @ 980 rpm must reconstruct the investigation's ~165 kN force (got {force_f4:.0})"
    );
    assert!(
        force_f4 - demand < 0.0,
        "F4 must be in reserve deficit on 20° ({force_f4:.0} - {demand:.0})"
    );
    assert!(
        force_f3 - demand >= margin,
        "F3 must clear the reserve margin ({force_f3:.0} - {demand:.0} >= {margin:.0})"
    );
}

/// Stage C composes reserve with (rather than replacing) the established upshift gates.
/// With zero window and isolated bands, the slope investigation's DERIVED operating point
/// puts F4 at 980 rpm and F3 just above the test's up band. It therefore shifts on flat
/// ground. Under the DERIVED 191.2 kN 20-degree load, F4's reserve is below the DERIVED
/// 10% + 10 kN policy margin, so the otherwise-identical upshift is vetoed.
#[test]
fn grade_reserve_veto_blocks_f3_to_f4_on_20_degrees() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 0;
    tp.shift_up_rpm = 980.0 * tp.gears_fwd[2] / tp.gears_fwd[3] - 1.0;
    tp.shift_down_rpm = 800.0;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let shaft = 980.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[3];

    let mut flat = TransmissionState {
        gear: 3,
        ..fresh(&tp)
    };
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut flat,
        &input(1.0, 0.0, [shaft, shaft], [0.0, 0.0]),
    );
    assert_eq!(
        flat.gear, 4,
        "the accepted flat-ground upshift must stay intact"
    );

    let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
    let mut grade = TransmissionState {
        gear: 3,
        ..fresh(&tp)
    };
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut grade,
        &input(1.0, 0.0, [shaft, shaft], [demand / 2.0; 2]),
    );
    assert_eq!(
        grade.gear, 3,
        "the 20-degree reserve deficit must veto F3 -> F4"
    );
}

/// A reserve deficit must persist for the full 13 DERIVED decision ticks. Warm the DERIVED
/// eight-tick EMA on flat ground, inject a 12-tick DERIVED 20-degree demand spike at an F5 operating point where F5 is
/// deficient but F4 is capable, then remove it. Filtering plus confirmation must reject the
/// transient without commanding any shift.
#[test]
fn transient_reserve_deficit_shorter_than_confirmation_does_not_downshift() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 0;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let shaft = 1500.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[4];
    let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
    let mut st = TransmissionState {
        gear: 5,
        ..fresh(&tp)
    };

    for _ in 0..32 {
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [shaft; 2], [0.0; 2]),
        );
    }
    for _ in 0..(GRADE_CONFIRM_TICKS - 1) {
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
        );
    }
    for _ in 0..32 {
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [shaft; 2], [0.0; 2]),
        );
    }

    assert_eq!(
        st.gear, 5,
        "a sub-confirmation load spike must not downshift"
    );
    assert_eq!(
        st.grade_confirm_ticks, 0,
        "the cleared deficit resets confirmation"
    );
}

/// The scheduler names one capability target; addressing changes only how it is executed.
/// This custom band setting isolates the reserve path at Tiger F6 = 600 rpm DERIVED test input under the
/// DERIVED 20-degree demand: F4 lacks margin, F3 clears it, and F2 would over-rev. Direct
/// commits F6 -> F3 in one event; Sequential pays F6 -> F5 first and holds F3 across the
/// remaining windows.
#[test]
fn direct_and_sequential_execute_the_same_grade_target_differently() {
    let mut base = tiger_tp();
    base.shift_down_rpm = 0.0;
    base.shift_ticks = 2;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let shaft = 600.0 * RPM_TO_RAD * base.sprocket_radius / base.gears_fwd[5];
    let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
    let seeded = |tp: &TransmissionParams| TransmissionState {
        gear: 6,
        demand_n: demand,
        demand_initialized: true,
        grade_confirm_ticks: GRADE_CONFIRM_TICKS - 1,
        ..fresh(tp)
    };

    let mut direct_tp = base.clone();
    direct_tp.shift_addressing = ShiftAddressing::Direct;
    let mut direct = seeded(&direct_tp);
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&direct_tp),
        &mut direct,
        &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
    );
    assert_eq!(
        direct.gear, 3,
        "Direct must commit straight to the legal target"
    );
    assert_eq!(
        direct.scheduler,
        SchedulerState::GradeShift { from: 6, to: 3 }
    );

    let mut sequential_tp = base;
    sequential_tp.shift_addressing = ShiftAddressing::Sequential;
    let mut sequential = seeded(&sequential_tp);
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&sequential_tp),
        &mut sequential,
        &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
    );
    assert_eq!(
        sequential.gear, 5,
        "Sequential may move only one adjacent gear"
    );
    assert_eq!(
        sequential.grade_target, 3,
        "Sequential must retain the F3 target"
    );
    assert_eq!(
        sequential.scheduler,
        SchedulerState::GradeShift { from: 6, to: 3 }
    );

    for _ in 0..8 {
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&sequential_tp),
            &mut sequential,
            &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
        );
    }
    assert_eq!(
        sequential.gear, 3,
        "Sequential must eventually reach the held target"
    );
}

/// Direct addressing never bypasses the signed landing gate. The same F6 -> F3 reserve target
/// as the addressing test is presented with the 20-tick DERIVED window; freezing the DERIVED
/// 20-degree reaction through that cut predicts `landing_m < 0`, so no grade shift may commit.
#[test]
fn direct_skip_refuses_a_predicted_backward_landing() {
    let mut tp = tiger_tp();
    tp.shift_down_rpm = 0.0;
    tp.shift_ticks = 20;
    tp.shift_addressing = ShiftAddressing::Direct;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let shaft = 600.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[5];
    let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
    let mut st = TransmissionState {
        gear: 6,
        demand_n: demand,
        demand_initialized: true,
        grade_confirm_ticks: GRADE_CONFIRM_TICKS - 1,
        ..fresh(&tp)
    };

    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
    );
    assert_eq!(
        st.gear, 6,
        "a sign-flipped direct landing must hold the engaged gear"
    );
    assert_eq!(st.shift_ticks, 0, "no interruption window may start");
    assert_eq!(st.scheduler, SchedulerState::Normal);
}

/// Hill hold is a stateful use of the existing brake law, not an extra force. At rest on the
/// DERIVED Tiger 20-degree load, F5 has negative reserve, so held W engages the flag and
/// Direct-addresses capable F3 while the full service-brake envelope keeps both belts stopped.
/// Once the shift ends, F3 transmits more than demand + margin; the hold releases and the same
/// tick begins a forward launch. Releasing W always clears the flag.
#[test]
fn hill_hold_engages_selects_launch_gear_and_releases_on_capability() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 2;
    tp.shift_addressing = ShiftAddressing::Direct;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
    let mut st = TransmissionState {
        gear: 5,
        ..fresh(&tp)
    };

    let first = step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [0.0; 2], [demand / 2.0; 2]),
    );
    assert!(
        st.hill_hold,
        "negative launch reserve must engage hill hold"
    );
    assert_eq!(
        st.gear, 3,
        "the hold must Direct-address the capable launch gear"
    );
    assert_eq!(st.scheduler, SchedulerState::HillHold);
    assert_eq!(
        first.next_speeds, [0.0; 2],
        "the modeled brakes hold through the cut"
    );

    let mut released = None;
    let mut speeds = first.next_speeds;
    for tick in 1..8 {
        let report = step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, speeds, [demand / 2.0; 2]),
        );
        speeds = report.next_speeds;
        if !st.hill_hold {
            released = Some(tick);
            break;
        }
    }
    assert!(
        released.is_some(),
        "capable F3 must release the hold after its window"
    );
    assert!(
        speeds[0] > 0.0 && speeds[1] > 0.0,
        "release must begin a forward launch"
    );
    assert_eq!(st.scheduler, SchedulerState::Normal);

    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(0.0, 0.0, speeds, [demand / 2.0; 2]),
    );
    assert!(!st.hill_hold, "command release always disengages hill hold");
}

/// D1 regression: a latched hold is a live capability decision, not a one-shot edge. A demand
/// sample that makes a lower gear capable must clear the stale GRADE LIMIT state and retarget.
#[test]
fn hill_hold_rechecks_capability_while_latched() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 0;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
    let mut st = TransmissionState {
        gear: 5,
        demand_n: demand,
        demand_initialized: true,
        scheduler: SchedulerState::GradeLimit,
        hill_hold: true,
        ..fresh(&tp)
    };

    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [0.0; 2], [demand / 2.0; 2]),
    );

    assert_eq!(st.gear, 3, "the live hold must retarget the capable F3");
    assert_ne!(
        st.scheduler,
        SchedulerState::GradeLimit,
        "a capable gear must clear stale GRADE LIMIT truth"
    );
}

/// D1 regression: if the selected launch gear has non-negative reserve but cannot clear the
/// full scheduler margin, transmitting its modeled force must still release the hold. The
/// release margin is deliberately smaller than the selection margin in this case.
#[test]
fn hill_hold_margin_short_capable_gear_can_release() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 0;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let k = tp.gears_fwd[0] / tp.sprocket_radius;
    tp.clutch_capacity = 2.0 * fp.engine_force / k;
    let modeled = available_force_in_gear(&tp, &fp, 0.0, tp.gears_fwd[0]);
    let demand = modeled - 10_000.0;
    assert!(
        modeled - demand < reserve_margin(demand),
        "fixture gear must be capable but margin-short"
    );
    let mut st = TransmissionState {
        gear: 1,
        omega_e: tp.engine.idle_rpm * RPM_TO_RAD,
        demand_n: demand,
        demand_initialized: true,
        scheduler: SchedulerState::HillHold,
        hill_hold: true,
        ..fresh(&tp)
    };

    let report = step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [0.0; 2], [demand / 2.0; 2]),
    );

    assert!(
        report.forces[0] + report.forces[1] >= demand,
        "fixture must transmit at least demand"
    );
    assert!(!st.hill_hold, "a capable margin-short gear must release");
}

/// D1c regression: a successful handoff suppresses near-rest relatching for the fixed cooldown,
/// but genuine backward motion beyond the engagement threshold overrides it immediately.
#[test]
fn hill_hold_release_cooldown_yields_to_real_rollback() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 0;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let k = tp.gears_fwd[0] / tp.sprocket_radius;
    tp.clutch_capacity = 2.0 * fp.engine_force / k;
    let modeled = available_force_in_gear(&tp, &fp, 0.0, tp.gears_fwd[0]);
    let release_demand = modeled - 10_000.0;
    let mut st = TransmissionState {
        omega_e: tp.engine.idle_rpm * RPM_TO_RAD,
        demand_n: release_demand,
        demand_initialized: true,
        scheduler: SchedulerState::HillHold,
        hill_hold: true,
        ..fresh(&tp)
    };

    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [0.0; 2], [release_demand / 2.0; 2]),
    );
    assert!(!st.hill_hold);
    assert_eq!(st.hold_reengage_ticks, HOLD_REENGAGE_TICKS);

    let deficit_demand = modeled + 100_000.0;
    st.demand_n = deficit_demand;
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [0.0; 2], [deficit_demand / 2.0; 2]),
    );
    assert!(!st.hill_hold, "near-rest chatter must respect the cooldown");

    st.demand_n = deficit_demand;
    let rollback = -(HILL_HOLD_ENGAGE_SPEED + 0.01);
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [rollback; 2], [deficit_demand / 2.0; 2]),
    );
    assert!(st.hill_hold, "a real rollback must override the cooldown");
}

fn correction_priority_fixture() -> (ForceParams, TransmissionParams, f32, f32) {
    let mut fp = lab_fp();
    fp.engine_force = 1_000_000.0;
    let mut tp = lab_tp();
    tp.shift_ticks = 0;
    tp.engine.governed_rpm = 4_000.0;
    tp.engine.torque_nm = vec![
        (0.0, 100.0),
        (1_100.0, 2_000.0),
        (1_800.0, 100.0),
        (2_900.0, 2_000.0),
        (4_000.0, 2_000.0),
    ];
    let shaft = 1_800.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[1];
    let current = available_force_in_gear(&tp, &fp, shaft, tp.gears_fwd[1]);
    let lower = available_force_in_gear(&tp, &fp, shaft, tp.gears_fwd[0]);
    let upper = available_force_in_gear(&tp, &fp, shaft, tp.gears_fwd[2]);
    let demand = current + 12_000.0;
    assert!(lower - demand >= reserve_margin(demand));
    assert!(upper - demand >= reserve_margin(demand));
    (fp, tp, shaft, demand)
}

/// D2 regression: a threshold-confirmed reserve deficit is a correction and must beat an
/// otherwise-valid above-band upshift preference on the same decision tick.
#[test]
fn confirmed_deficit_precedes_upshift_arm() {
    let (fp, tp, shaft, demand) = correction_priority_fixture();
    let mut st = TransmissionState {
        gear: 2,
        demand_n: demand,
        demand_initialized: true,
        grade_confirm_ticks: GRADE_CONFIRM_TICKS - 1,
        ..fresh(&tp)
    };

    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
    );

    assert_eq!(st.gear, 1, "confirmed deficit must correct downward");
}

/// D5 regression: reversal dwell protects band preferences from hunting; it must not block the
/// correction after the full reserve-deficit confirmation has already been paid.
#[test]
fn confirmed_deficit_bypasses_reversal_dwell() {
    let (fp, mut tp, shaft, demand) = correction_priority_fixture();
    tp.shift_up_rpm = 10_000.0;
    let mut st = TransmissionState {
        gear: 2,
        last_shift_dir: 1,
        dwell_ticks: REVERSAL_DWELL_TICKS,
        demand_n: demand,
        demand_initialized: true,
        grade_confirm_ticks: GRADE_CONFIRM_TICKS - 1,
        ..fresh(&tp)
    };

    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
    );

    assert_eq!(
        st.gear, 1,
        "confirmed correction must ignore reversal dwell"
    );
}

/// D3 regression: every sequential continuation is selected again. Releasing propulsive intent
/// cancels the held target instead of paying another stale adjacent shift window.
#[test]
fn sequential_target_cancels_when_propulsive_intent_releases() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 0;
    tp.shift_addressing = ShiftAddressing::Sequential;
    let fp = lab_fp();
    let shaft = 1_700.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[4];
    let mut st = TransmissionState {
        gear: 5,
        demand_initialized: true,
        grade_target: 3,
        scheduler: SchedulerState::GradeShift { from: 6, to: 3 },
        ..fresh(&tp)
    };

    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(0.0, 0.0, [shaft; 2], [0.0; 2]),
    );

    assert_eq!(st.gear, 5, "released intent must not continue the cascade");
    assert_eq!(st.grade_target, 0);
    assert_eq!(st.scheduler, SchedulerState::Normal);
}

/// D3 regression: a held sequential target also cancels when the filtered demand recovers and
/// the current gear is no longer deficient, even if the driver continues holding throttle.
#[test]
fn sequential_target_cancels_when_demand_recovers() {
    let (fp, mut tp, shaft, _) = correction_priority_fixture();
    tp.shift_addressing = ShiftAddressing::Sequential;
    tp.shift_up_rpm = 10_000.0;
    tp.shift_down_rpm = 0.0;
    let mut st = TransmissionState {
        gear: 2,
        demand_initialized: true,
        grade_target: 1,
        scheduler: SchedulerState::GradeShift { from: 3, to: 1 },
        ..fresh(&tp)
    };

    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [shaft; 2], [0.0; 2]),
    );

    assert_eq!(st.gear, 2, "recovered demand must not continue the cascade");
    assert_eq!(st.grade_target, 0);
    assert_eq!(st.scheduler, SchedulerState::Normal);
}

/// D3 / finding 8a regression: F- and R-ladder demand projections have opposite signs. A
/// direction swap must discard the old EMA and seed directly from the new ladder's sample.
#[test]
fn direction_swap_reseeds_demand_ema() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 0;
    let fp = lab_fp();
    let reverse_demand = 20_000.0;
    let mut st = TransmissionState {
        demand_n: 100_000.0,
        demand_initialized: true,
        ..fresh(&tp)
    };

    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(-1.0, 0.0, [0.0; 2], [-reverse_demand / 2.0; 2]),
    );

    assert!(st.reverse);
    assert_eq!(st.demand_n.to_bits(), reverse_demand.to_bits());
}

/// D8 regression: one capable sample decays accumulated evidence by one tick; it does not erase
/// twelve prior deficit samples. Two more deficit samples must therefore confirm the correction.
#[test]
fn reserve_confirmation_decays_across_one_tick_jitter() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 0;
    tp.shift_up_rpm = 10_000.0;
    tp.shift_down_rpm = 0.0;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    let shaft = 600.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[5];
    let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
    let mut st = TransmissionState {
        gear: 6,
        demand_initialized: true,
        ..fresh(&tp)
    };

    for _ in 0..(GRADE_CONFIRM_TICKS - 1) {
        st.demand_n = demand;
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
        );
    }
    st.demand_n = 0.0;
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [shaft; 2], [0.0; 2]),
    );
    for _ in 0..2 {
        st.demand_n = demand;
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
        );
    }

    assert!(
        st.gear < 6,
        "decayed evidence must still reach confirmation"
    );
}

/// D6 regression: a positive signed shaft driven beyond the governed range by a downhill load
/// receives a protective upshift with the throttle released, and the declutched crank slows.
#[test]
fn downhill_overrun_protective_upshift_lowers_crank_speed() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 2;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let overrun_rpm = tp.engine.governed_rpm + 200.0;
    let shaft = overrun_rpm * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[3];
    let mut st = TransmissionState {
        gear: 4,
        omega_e: overrun_rpm * RPM_TO_RAD,
        demand_initialized: true,
        ..fresh(&tp)
    };

    let report = step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(0.0, 0.0, [shaft; 2], [-40_000.0; 2]),
    );

    println!(
        "protective overrun: F4 shaft {overrun_rpm:.1} rpm -> F{}; crank {overrun_rpm:.1} -> {:.1} rpm",
        st.gear, report.rpm
    );
    assert_eq!(
        st.gear, 5,
        "overrun must protectively upshift while coasting"
    );
    assert!(
        report.rpm < overrun_rpm,
        "declutched crank must slow from {overrun_rpm:.1} rpm, got {:.1}",
        report.rpm
    );
}

/// The Governor adapter IS the legacy tail: per side, bit-equal to `governor_belt`.
#[test]
fn governor_adapter_matches_legacy_belt() {
    let fp = lab_fp();
    let tp = lab_tp();
    let mut st = fresh(&tp);
    let inp = input(0.7, 0.3, [4.2, -1.1], [23_000.0, -9_500.0]);
    let report = step(TransmissionMode::Governor, &fp, Some(&tp), &mut st, &inp);
    for i in 0..2 {
        let (engine, next) = forces::governor_belt(
            &fp,
            inp.side_commands[i],
            inp.speeds[i],
            inp.reactions[i],
            inp.dt,
        );
        assert_eq!(report.forces[i], engine);
        assert_eq!(report.next_speeds[i], next);
    }
    assert_eq!(st, fresh(&tp), "governor must not touch state");
}

/// Auto-shift: crossing the up band shifts up exactly once (the interruption window
/// blocks a second decision), the mid-band is quiet in both directions, and the down
/// band shifts down — the hysteresis gap is what kills hunting.
#[test]
fn gear_shift_hysteresis() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    // rpm(gear1) at m: m·G1/r_s in rad/s → rpm. G1 ≈ ω_rated·r_s/v1.
    let g1 = tp.gears_fwd[0];
    let m_for = |rpm: f32| rpm * RPM_TO_RAD * tp.sprocket_radius / g1;

    // Above the up band → one upshift, then the window holds further decisions.
    // 1780 rpm: comfortably past the band AND past the fix-1a landing gate (unloaded
    // landing 1780 × 8/12.7 ≈ 1121 rpm ≥ down band 950 + margin 150).
    let v = m_for(1_780.0);
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v, v], [0.0, 0.0]),
    );
    assert_eq!(st.gear, 2);
    assert!(st.shift_ticks > 0);
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v, v], [0.0, 0.0]),
    );
    assert_eq!(
        st.gear, 2,
        "no second decision inside the interruption window"
    );

    // Drain the window AND the fix-1b reversal dwell at a mid-band speed for gear 2:
    // no hunting either way.
    let g2 = tp.gears_fwd[1];
    let v_mid = 1_300.0 * RPM_TO_RAD * tp.sprocket_radius / g2;
    for _ in 0..(tp.shift_ticks as usize + REVERSAL_DWELL_TICKS as usize + 5) {
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [v_mid, v_mid], [0.0, 0.0]),
        );
    }
    assert_eq!(st.gear, 2);
    assert_eq!(st.shift_ticks, 0);

    // Below the down band → downshift.
    let v_low = 900.0 * RPM_TO_RAD * tp.sprocket_radius / g2;
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v_low, v_low], [0.0, 0.0]),
    );
    assert_eq!(st.gear, 1);
}

/// The shift is a torque interruption: propulsion force is zero for exactly
/// the authored `shift_secs` worth of ticks, then returns. (Throttle 1.0 keeps engine drag released, and
/// reactions are zero, so the per-side force IS the propulsion share.)
#[test]
fn shift_torque_interruption_window() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    let g1 = tp.gears_fwd[0];
    // 1780 rpm — past the up band and the fix-1a landing gate (see gear_shift_hysteresis).
    let v = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
    let inp = input(1.0, 0.0, [v, v], [0.0, 0.0]);
    let mut zero_ticks = 0;
    loop {
        let r = step(TransmissionMode::Hybrid, &fp, Some(&tp), &mut st, &inp);
        if r.shifting {
            assert_eq!(
                r.forces[0], 0.0,
                "torque must be interrupted through the shift"
            );
            assert_eq!(r.forces[1], 0.0);
            zero_ticks += 1;
        } else if zero_ticks > 0 {
            assert!(r.forces[0] > 0.0, "torque must return after the window");
            break;
        }
        assert!(zero_ticks <= tp.shift_ticks as usize, "window must end");
    }
    assert_eq!(zero_ticks, tp.shift_ticks as usize);
}

/// Fix-1a: the upshift commits only if the belt state PREDICTED at the end of the
/// torque-cut window still lands above the down band + POSTSHIFT_MARGIN_RPM. Same
/// operating point, two loads: unloaded the landing holds and the shift engages; under
/// a heavy frozen reaction (25 kN/side) the cut would bleed ≈ 0.98 m/s
/// (25 kN / 8 t × 20 ticks / 64 Hz) and land deep inside the down band — the shift
/// must be refused. Pre-fix, exactly this bleed fired the down band the tick the
/// freeze lifted: the measured 1-2-1-2 climb.
#[test]
fn upshift_landing_gate_blocks_shift_cut_hunting() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let g1 = tp.gears_fwd[0];
    let v = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
    let mut st = fresh(&tp);
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v, v], [25_000.0, 25_000.0]),
    );
    assert_eq!(
        st.gear, 1,
        "a landing predicted inside the down band must refuse the upshift"
    );
    let mut st = fresh(&tp);
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v, v], [0.0, 0.0]),
    );
    assert_eq!(st.gear, 2, "unloaded, the same operating point upshifts");
}

/// Stage A (signed shaft): a belt BACK-DRIVEN in a forward gear (m < 0, W held — the
/// backslide) commits NO shifts in either direction, and the engine keeps delivering
/// FORWARD drive (the governor must not cut). Pre-fix, `|m| = 2.5` in gear 1 read as
/// 2025 rpm: past the up band (ladder walk while sliding backward) AND past the
/// governed cut (torque → 0, so the tank back-slid under full W indefinitely). The
/// signed shaft reads −2025 rpm: the up band can never fire, the down band is held
/// (a backslide is not "running slow forward"), and the engine evaluates at the
/// non-negative rev floor, delivering forward force.
#[test]
fn backslide_holds_gear_and_keeps_forward_drive() {
    let (fp, tp) = (lab_fp(), lab_tp());
    // Up-band side: gear 1 at m = −2.5 under a grade-like reaction.
    let mut st = fresh(&tp);
    for tick in 0..96 {
        let rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [-2.5, -2.5], [40_000.0, 40_000.0]),
        );
        assert_eq!(
            st.gear, 1,
            "tick {tick}: a backslide must not walk the ladder"
        );
        assert_eq!(
            st.shift_ticks, 0,
            "tick {tick}: no shift may commit during a backslide"
        );
        assert!(
            rep.forces[0] > 0.0 && rep.forces[1] > 0.0,
            "tick {tick}: the engine must keep delivering FORWARD drive during a \
                 backslide — the governor must not cut on |shaft| (forces {:?})",
            rep.forces
        );
    }
    // Down-band side: gear 3 back-driven — the signed rpm sits under the down band,
    // but the backslide state HOLDS the engaged gear (no downshift walk either).
    let mut st = TransmissionState {
        gear: 3,
        ..fresh(&tp)
    };
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [-2.5, -2.5], [40_000.0, 40_000.0]),
    );
    assert_eq!(
        st.gear, 3,
        "a backslide must hold the engaged gear, not downshift-walk"
    );
    assert_eq!(st.shift_ticks, 0);
}

/// Stage A (signed landing gate): an upshift whose PREDICTED landing is sign-flipped
/// (backward) is always refused. The traced grade case: at 1780 rpm in gear 1 under a
/// frozen r_mean = 221 kN, the torque-cut window bleeds 221 kN / 8 t × 0.3125 s ≈
/// 8.6 m/s — landing ≈ −6.4 m/s, BACKWARD. Under `|m|` that read as ≈ 3280 rpm ≥
/// band + margin and the gate PASSED the catastrophic on-grade upshift; the signed
/// gate requires a POSITIVE landing shaft.
#[test]
fn landing_gate_refuses_sign_flipped_landing() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let g1 = tp.gears_fwd[0];
    let v = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / g1; // above the up band
    let mut st = fresh(&tp);
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v, v], [221_000.0, 221_000.0]),
    );
    assert_eq!(
        st.gear, 1,
        "a sign-flipped predicted landing must refuse the upshift"
    );
    assert_eq!(st.shift_ticks, 0, "no shift may have committed");
}

/// Stage-A review round: the REVERSE-ladder mirror of the backslide test. Driving in
/// R (dir = −1) while back-driven FORWARD (m > 0 → shaft = dir·m < 0): no shifts in
/// either direction, and the drive force stays R-SIGNED and non-zero (the governor
/// must not cut on |shaft| — pre-fix, |m| = 2.5 in R1 read 2025 rpm, past the
/// governed cut, torque → 0). Uses a 3-gear reverse ladder so "no shifts" actually
/// has shifts to refuse.
#[test]
fn reverse_backslide_holds_gear_and_keeps_reverse_drive() {
    let fp = lab_fp();
    let tp = TransmissionParams::from_authoring(&TransmissionAuthoring {
        idle_rpm: 600.0,
        governed_rpm: 1800.0,
        rated_rpm: 1800.0,
        torque_nm: &[
            (600.0, 1650.0),
            (1100.0, 2200.0),
            (1700.0, 1950.0),
            (1800.0, 0.0),
        ],
        forward_speeds_kmh: &[8.0, 12.7, 20.4, 32.6, 52.2],
        reverse_speeds_kmh: &[8.0, 12.7, 20.4],
        shift_up_rpm: 1700.0,
        shift_down_rpm: 950.0,
        steer_radii_m: &[
            (3.0, 8.9),
            (4.8, 14.2),
            (7.7, 22.8),
            (12.3, 36.4),
            (19.7, 58.3),
        ],
        steer_capacity_n: 240_000.0,
        recirculation: 0.9,
        brake_capacity_n: 120_000.0,
        brake_static_factor: 1.6,
        drag_fraction: 0.25,
        engine_inertia_kgm2: 4.0,
        clutch_capacity_nm: 2860.0,
        belt_inertia: 8_000.0,
        shift_secs: 0.31,
        shift_addressing: ShiftAddressing::Sequential,
        sprocket_radius_m: 0.34,
        half_tread_m: 1.25,
    })
    .expect("reverse-ladder test authoring must be valid");
    // Up-band mirror: R1 back-driven at m = +2.5 (|m| would read 2025 rpm — ladder
    // walk + governed cut pre-fix). Held S (reverse throttle), grade-like reaction.
    let mut st = TransmissionState {
        reverse: true,
        ..fresh(&tp)
    };
    for tick in 0..96 {
        let rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(-1.0, 0.0, [2.5, 2.5], [-40_000.0, -40_000.0]),
        );
        assert!(st.reverse, "tick {tick}: the R ladder stays engaged");
        assert_eq!(
            st.gear, 1,
            "tick {tick}: a reverse backslide must not walk the R ladder"
        );
        assert_eq!(st.shift_ticks, 0, "tick {tick}: no shift may commit");
        assert!(
            rep.forces[0] < 0.0 && rep.forces[1] < 0.0,
            "tick {tick}: the engine must keep delivering R-SIGNED drive during a \
                 reverse backslide (forces {:?})",
            rep.forces
        );
    }
    // Down-band mirror: R2 back-driven slowly forward (shaft = −0.3, a genuine slide
    // past the at-rest threshold) — the backslide state holds the engaged gear.
    let mut st = TransmissionState {
        gear: 2,
        reverse: true,
        ..fresh(&tp)
    };
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(-1.0, 0.0, [0.3, 0.3], [0.0, 0.0]),
    );
    assert_eq!(
        st.gear, 2,
        "a reverse backslide must hold the engaged gear, not downshift-walk"
    );
    assert_eq!(st.shift_ticks, 0);
}

/// Stage B: the HUD readout reports the CRANK STATE ω_e directly — the state IS the
/// display. Freshly constructed, it reads idle; driving in reverse it reads the
/// crank's geared speed with the R label; back-driven forward while in R (the stage-A
/// scenario), the crank cannot follow the negative shaft — the stall guard keeps ω_e
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
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
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
    // the stall guard bounds it at idle − STALL_GUARD_BAND_RPM, and the readout shows
    // the honest idle-ish crank, not a fake geared rpm.
    let mut st = TransmissionState {
        reverse: true,
        ..fresh(&tp)
    };
    for _ in 0..64 {
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(-1.0, 0.0, [2.0, 2.0], [-40_000.0, -40_000.0]),
        );
    }
    let r = readout(&st, &tp);
    assert_eq!(r.gear_label, "R1");
    assert!(
        r.rpm >= tp.engine.idle_rpm - STALL_GUARD_BAND_RPM - 1.0 && r.rpm <= tp.engine.governed_rpm,
        "a back-driven R shaft must read the idle-ish crank (≥ idle − band), got {:.0}",
        r.rpm
    );
}

/// Stage-A review round (FIX 1 regression): coasting to rest in a cruise gear must
/// complete the downshift chain to gear 1. The brake stop-force/integration order
/// leaves a stable numerical residual at rest (measured ≈ −1.7e−9 m/s: Hybrid, gear
/// 3, zero command, 20 kN/side reaction) — a hard `shaft >= 0` backslide guard read
/// that residual as "back-driven" and stranded the box in gear 3 forever. The guard's
/// −PARK_ENGAGE_SPEED threshold lets numerical rest downshift normally.
#[test]
fn coast_to_rest_completes_downshift_chain() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = TransmissionState {
        gear: 3,
        ..fresh(&tp)
    };
    let mut speeds = [-1.0e-5f32, -1.0e-5];
    for _ in 0..256 {
        let rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, speeds, [20_000.0, 20_000.0]),
        );
        speeds = rep.next_speeds;
    }
    assert!(
        speeds[0].abs() < PARK_ENGAGE_SPEED && speeds[1].abs() < PARK_ENGAGE_SPEED,
        "the scenario must actually be at (numerical) rest, got {speeds:?}"
    );
    assert!(st.park, "zero command at rest must have latched the park");
    assert_eq!(
        st.gear, 1,
        "coasting to rest must complete the downshift chain, not strand the cruise \
             gear behind the backslide guard"
    );
}

/// Fix-1b: after a shift commits, the OPPOSITE-direction shift is dwell-blocked for
/// REVERSAL_DWELL_TICKS, but SAME-direction shifts stay free (a rapid 1-2-3 climb must
/// not slow down).
#[test]
fn dwell_blocks_reversal_not_same_direction() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let rpm_v = |rpm: f32, g: f32| rpm * RPM_TO_RAD * tp.sprocket_radius / g;
    let mut st = fresh(&tp);
    let at = |st: &mut TransmissionState, v: f32| {
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            st,
            &input(1.0, 0.0, [v, v], [0.0, 0.0]),
        );
    };
    // 1 → 2 commits (dwell armed).
    at(&mut st, rpm_v(1_780.0, tp.gears_fwd[0]));
    assert_eq!(st.gear, 2);
    // Drain the interruption window at a mid-band gear-2 speed; the dwell (32 ticks)
    // must still be live when the window (≈ 20 ticks) ends, or this test bites nothing.
    for _ in 0..tp.shift_ticks {
        at(&mut st, rpm_v(1_300.0, tp.gears_fwd[1]));
    }
    assert_eq!(st.gear, 2);
    assert!(st.dwell_ticks > 0, "the dwell must outlive the window");
    // SAME direction: 2 → 3 engages immediately despite the live dwell (1780 rpm is
    // past the up band, landing 1780 × 12.7/20.4 ≈ 1108 ≥ 1100 clears the fix-1a gate).
    at(&mut st, rpm_v(1_780.0, tp.gears_fwd[1]));
    assert_eq!(
        st.gear, 3,
        "same-direction shifts must not be dwell-blocked"
    );
    // OPPOSITE direction: drop below gear-3's down band. The downshift must wait out
    // the FULL dwell after the window — the dwell counts only outside the frozen
    // window (review round), so the reversal engages exactly at
    // window + REVERSAL_DWELL_TICKS.
    let v_low = rpm_v(900.0, tp.gears_fwd[2]);
    let mut ticks = 0usize;
    while st.gear == 3 {
        at(&mut st, v_low);
        ticks += 1;
        assert!(ticks < 200, "the downshift must eventually engage");
    }
    assert_eq!(
        ticks,
        tp.shift_ticks as usize + REVERSAL_DWELL_TICKS as usize,
        "the reversal must get the full post-engagement dwell (window {} + dwell {})",
        tp.shift_ticks,
        REVERSAL_DWELL_TICKS
    );
}

/// Review round (intent gate): upshifts are considered only under PROPULSIVE drive. A
/// braking (opposing-throttle) or coasting driver at high rpm never needs one — and
/// the landing predictor has no brake term, so consulting it there produced a false
/// shift (predicted 1652 rpm on drag alone vs 1262 real under the brakes) followed by
/// a reversal cycle.
#[test]
fn no_upshift_while_braking_or_coasting() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let g1 = tp.gears_fwd[0];
    let v = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
    for throttle in [0.0, -1.0] {
        let mut st = fresh(&tp);
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(throttle, 0.0, [v, v], [0.0, 0.0]),
        );
        assert_eq!(
            st.gear, 1,
            "throttle {throttle}: no upshift without propulsive drive"
        );
        assert_eq!(st.shift_ticks, 0, "throttle {throttle}: no shift committed");
    }
}

/// Review round (predictor-domain guard): while the L600 steering detent is engaged
/// the landing predictor has no λ/steer state, so upshifts are DEFERRED until the
/// detent releases; downshifts stay allowed mid-turn.
#[test]
fn l600_detent_defers_upshift() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let v_up = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[0];
    // Detent engaged (tight) at an above-band operating point: upshift deferred.
    let mut st = TransmissionState {
        steer_step: 2,
        ..fresh(&tp)
    };
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 1.0, [v_up, v_up], [0.0, 0.0]),
    );
    assert_eq!(st.gear, 1, "detent-active upshift must be deferred");
    // Same operating point, detent released: the upshift proceeds — it is the detent
    // that defers, not the operating point.
    let mut st = fresh(&tp);
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v_up, v_up], [0.0, 0.0]),
    );
    assert_eq!(st.gear, 2, "detent released, the upshift proceeds");
    // Downshifts stay allowed mid-turn (over-rev gate permitting).
    let v_low = 900.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[2];
    let mut st = TransmissionState {
        gear: 3,
        steer_step: 2,
        ..fresh(&tp)
    };
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 1.0, [v_low, v_low], [0.0, 0.0]),
    );
    assert_eq!(st.gear, 2, "downshifts stay allowed during a detent turn");
}

/// Review round (fix B): releasing the steer at a standstill pivot must actively
/// ARREST the belt difference — with zero ground reactions (airborne), only the servo
/// can. The |m|-only blend weight zeroed both force terms at steer = 0 (w = 1,
/// pivot_f = 0), leaving the belts counter-rotating forever; the steer-scaled weight
/// returns the released stick to the curvature servo, whose target is 0.
#[test]
fn hybrid_steer_release_arrests_pivot() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    let mut speeds = [0.0f32; 2];
    // Spin up a standstill pivot (zero reactions — the worst case: nothing external
    // ever damps the belts).
    for _ in 0..64 {
        let rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 1.0, speeds, [0.0, 0.0]),
        );
        speeds = rep.next_speeds;
    }
    let d0 = (speeds[0] - speeds[1]) / 2.0;
    assert!(d0 > 0.1, "the pivot must actually be turning (d = {d0})");
    // Release the steer: d must decay to ~0 within a bounded window.
    for _ in 0..32 {
        let rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, speeds, [0.0, 0.0]),
        );
        speeds = rep.next_speeds;
    }
    let d1 = (speeds[0] - speeds[1]) / 2.0;
    assert!(
        d1.abs() < 0.01,
        "released steer must arrest the pivot (d {d0} -> {d1})"
    );
}

/// Fix-1c: a downshift whose landing rpm would exceed the engine's max curve rpm minus
/// OVERREV_MARGIN_RPM is refused. Custom two-gear ladder with a 2.55 ratio step (a
/// shape the spec-level hysteresis validation would reject — deliberately extreme to
/// make the gate the ONLY thing standing between the down band and a 2295-rpm landing
/// on an 1800-rpm curve): at 900 rpm in gear 2 (below the 950 down band) the landing
/// in gear 1 ≈ 2295 > 1800 − 100 → refused; at 600 rpm the landing ≈ 1530 is inside
/// the envelope and the downshift proceeds.
#[test]
fn overrev_gate_refuses_too_early_downshift() {
    let fp = lab_fp();
    let mut tp = TransmissionParams::from_authoring(&TransmissionAuthoring {
        idle_rpm: 600.0,
        governed_rpm: 1800.0,
        rated_rpm: 1800.0,
        torque_nm: &[
            (600.0, 1650.0),
            (1100.0, 2200.0),
            (1700.0, 1950.0),
            (1800.0, 0.0),
        ],
        forward_speeds_kmh: &[8.0, 20.4],
        reverse_speeds_kmh: &[8.0],
        shift_up_rpm: 1700.0,
        // Validate the authoring shape before introducing the test's deliberate runtime-only
        // invalidity below.
        shift_down_rpm: 600.0,
        steer_radii_m: &[(3.0, 8.9), (7.7, 22.8)],
        steer_capacity_n: 240_000.0,
        recirculation: 0.9,
        brake_capacity_n: 120_000.0,
        brake_static_factor: 1.6,
        drag_fraction: 0.25,
        engine_inertia_kgm2: 4.0,
        clutch_capacity_nm: 2860.0,
        belt_inertia: 8_000.0,
        shift_secs: 0.31,
        shift_addressing: ShiftAddressing::Sequential,
        sprocket_radius_m: 0.34,
        half_tread_m: 1.25,
    })
    .expect("over-rev fixture starts from valid authoring");
    tp.shift_down_rpm = 950.0;
    let g2 = tp.gears_fwd[1];
    let mut st = TransmissionState {
        gear: 2,
        ..fresh(&tp)
    };
    let v = 900.0 * RPM_TO_RAD * tp.sprocket_radius / g2;
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v, v], [0.0, 0.0]),
    );
    assert_eq!(
        st.gear, 2,
        "a landing past max curve rpm − margin must refuse the downshift"
    );
    let v = 600.0 * RPM_TO_RAD * tp.sprocket_radius / g2;
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v, v], [0.0, 0.0]),
    );
    assert_eq!(st.gear, 1, "an in-envelope landing must downshift");
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
        let inp = input(0.5, 1.0, speeds, [0.0, 0.0]);
        last = step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
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

/// At the Tiger's F8 cruise, the WIDE fixed-radius differential legitimately puts the
/// outer belt above the vehicle's mean-axis speed limit. The transmission must preserve
/// that authored `d = kappa * m` instead of clipping the outer belt back to `max_speed`.
#[test]
fn tiger_f8_wide_outer_belt_exceeds_mean_speed_limit() {
    let tp = tiger_tp();
    let mut fp = lab_fp();
    fp.max_speed = 10.5;
    let half_tread_m = 1.4904f32;
    let cruise_m = 10.49f32;
    let mut st = TransmissionState {
        gear: 8,
        omega_e: cruise_m * tp.gears_fwd[7] / tp.sprocket_radius,
        ..fresh(&tp)
    };
    let rep = step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.3, [cruise_m, cruise_m], [0.0, 0.0]),
    );

    assert_eq!(rep.steer_step, 1, "0.3 steer must engage the WIDE detent");
    let m = (rep.next_speeds[0] + rep.next_speeds[1]) / 2.0;
    let d = (rep.next_speeds[0] - rep.next_speeds[1]) / 2.0;
    let expected_d = tp.steer_kappa[7].1 * m.abs();
    assert!(
        m <= fp.max_speed + 1e-5,
        "F8 cruise mean speed must remain bounded at max_speed (m {m}, limit {})",
        fp.max_speed,
    );
    assert!(
        rep.next_speeds[0] > fp.max_speed,
        "F8 WIDE outer belt must exceed the {limit} m/s mean-axis limit by its kinematic \
             differential (belts {:?}, m {m}, d {d})",
        rep.next_speeds,
        limit = fp.max_speed,
    );
    let outer_excess = rep.next_speeds[0] - m;
    assert!(
        (d - expected_d).abs() <= 0.02 * expected_d
            && (outer_excess - expected_d).abs() <= 0.02 * expected_d,
        "F8 WIDE must preserve outer = m + d with d = kappa*m \
             (d {d}, outer excess {outer_excess}, expected {expected_d})"
    );
    let belt_radius = half_tread_m / (d / m.abs());
    assert!(
        (belt_radius - 165.0).abs() <= 0.02 * 165.0,
        "F8 WIDE belt-kinematic radius must stay within 2% of the authored 165 m \
             (got {belt_radius})"
    );
}

/// Codex-4 regression: a tick that carries `m` through zero must not project the
/// constraint onto the pre-tick |m| branch — that enforces `d = s·κ·m` on the wrong
/// side of the cusp, flipping `d` AGAINST the commanded steer for a tick (a yaw
/// impulse, and ringing if m chatters around zero). Codex's scenario: slow forward
/// roll, tight detent, strong equal reactions during a shift interruption — the tick
/// lands m well negative; `d` must stay on the steer's side.
#[test]
fn l600_constraint_survives_m_zero_crossing() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = TransmissionState {
        steer_step: 2,
        shift_ticks: 5,
        ..fresh(&tp)
    };
    let (m0, d0) = (0.100f32, 0.043);
    let inp = input(0.5, 1.0, [m0 + d0, m0 - d0], [250_000.0, 250_000.0]);
    let rep = step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
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

/// Steering detent hysteresis: the tight step engages at ≥ TIGHT_ON and releases only
/// below TIGHT_OFF (the |steer| ≥ 0.5 design threshold, hysteresis-wrapped).
#[test]
fn steer_step_hysteresis() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    let mut at = |steer: f32| {
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.5, steer, [2.0, 2.0], [0.0, 0.0]),
        );
        st.steer_step
    };
    assert_eq!(at(0.10), 0, "below WIDE_ON stays straight");
    assert_eq!(at(0.30), 1, "wide engages");
    assert_eq!(
        at(0.50),
        1,
        "0.5 is inside the tight hysteresis band from below"
    );
    assert_eq!(at(0.60), 2, "tight engages at ≥ TIGHT_ON");
    assert_eq!(at(0.50), 2, "0.5 holds tight from above");
    assert_eq!(at(0.40), 1, "below TIGHT_OFF releases to wide");
    assert_eq!(at(0.02), 0, "below WIDE_OFF releases to straight");
}

/// Static breakaway and dynamic dissipation are separate capacities. A parked belt inside the
/// multiplied static capacity holds EXACTLY (v̇ = 0, bit-zero); demand past it back-drives.
#[test]
fn brake_capacity_breach_backdrives() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    // Above dynamic but inside static: R = 1.5·B_dynamic < 1.6·B_dynamic, zero command, zero
    // speed -> exact hold.
    let r_in = 1.5 * tp.brake_capacity_n;
    let rep = step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(0.0, 0.0, [0.0, 0.0], [r_in, r_in]),
    );
    assert_eq!(
        rep.next_speeds,
        [0.0, 0.0],
        "inside static capacity the parked brake holds exactly"
    );
    // Past static capacity: R = 1.7·B_dynamic > 1.6·B_dynamic -> honest back-drive.
    let r_out = 1.7 * tp.brake_capacity_n;
    let rep = step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(0.0, 0.0, [0.0, 0.0], [r_out, r_out]),
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
        brake_capacity_for_regime(&tp, true, 0.0, 0.0),
        static_capacity,
        "a latched belt strictly inside the at-rest band gets static breakaway capacity"
    );
    assert_eq!(
        brake_capacity_for_regime(&tp, false, 0.0, 0.0),
        dynamic,
        "an unlatched settle envelope stays dynamic"
    );
    assert_eq!(
        brake_capacity_for_regime(&tp, true, 1.0, 0.0),
        dynamic,
        "service braking stays dynamic even if stale latch state is present"
    );
    assert_eq!(
        brake_capacity_for_regime(&tp, true, 0.0, PARK_ENGAGE_SPEED),
        dynamic,
        "leaving the strict at-rest band drops the cap that same tick"
    );
}

/// Codex-2 regression, half 1: the parking brake SETTLES creep instead of freezing it.
/// The old `B = clamp(R − Q, ±cap)` at a small positive belt speed with `R > Q` set
/// `v̇ = 0` exactly — positive brake work cancelling grip and drag, preserving creep
/// forever. The stop-force law lands the belt at zero.
#[test]
fn parking_brake_settles_creep() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    // Creep below the latch threshold, zero command, a ground reaction R > Q inside
    // capacity (codex's exact configuration).
    let rep = step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
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

/// Codex-2 regression, half 2: past a capacity breach the latched parking brake stays
/// SATURATED against the slide — the blend-only envelope faded to zero once the
/// back-driven belt passed `slip_saturation`, releasing the brake exactly when it was
/// needed. The latched brake keeps rubbing at `B_max` however fast the belt slides.
#[test]
fn parking_brake_stays_saturated_past_breach() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    let r_breach = 1.7 * tp.brake_capacity_n;
    let mut speeds = [0.0f32; 2];
    let mut last = TransmissionReport::default();
    for _ in 0..30 {
        let inp = input(0.0, 0.0, speeds, [r_breach, r_breach]);
        last = step(TransmissionMode::Hybrid, &fp, Some(&tp), &mut st, &inp);
        speeds = last.next_speeds;
    }
    assert!(
        st.park,
        "the latch must not release without a drive command"
    );
    assert!(
        speeds[0] < -fp.slip_saturation,
        "the breach must back-drive the belt well past the blend's fade band \
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
    let moving = input(0.0, 0.0, [-fp.slip_saturation; 2], [r_breach; 2]);
    let mut dynamic_tp = tp.clone();
    dynamic_tp.brake_static_factor = 1.0;
    let mut static_state = TransmissionState {
        park: true,
        ..fresh(&tp)
    };
    let mut dynamic_state = static_state;
    let static_report = step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut static_state,
        &moving,
    );
    let dynamic_report = step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&dynamic_tp),
        &mut dynamic_state,
        &moving,
    );
    assert_eq!(
        static_report.forces, dynamic_report.forces,
        "a moving latched slide must drop to dynamic brake capacity"
    );
    assert_eq!(
        static_report.next_speeds, dynamic_report.next_speeds,
        "the post-breach slide must be bit-identical to factor-1.0 dynamic braking"
    );
}

/// Discrete passivity of the whole brake stack: against a brakeless baseline
/// (`grip_stiffness = 0` disables park/hold; same drag, same drive), the brake's
/// contribution over one tick never pushes the belt PAST the baseline in its direction
/// of motion, never reverses it through zero, and never increases |v_next| beyond the
/// baseline's. Swept over speeds and reactions on both sides of capacity, latched and
/// unlatched.
#[test]
fn brake_is_discretely_passive() {
    let tp = lab_tp();
    let fp_braked = lab_fp();
    let mut fp_free = lab_fp();
    fp_free.grip_stiffness = 0.0;
    for park in [false, true] {
        for v in [-0.6f32, -0.2, -0.03, 0.0, 0.03, 0.2, 0.6] {
            for r in [-1.5f32, -0.5, 0.0, 0.5, 1.5] {
                let r = r * tp.brake_capacity_n;
                let inp = input(0.0, 0.0, [v, v], [r, r]);
                let mut st_b = TransmissionState { park, ..fresh(&tp) };
                let braked = step(
                    TransmissionMode::Hybrid,
                    &fp_braked,
                    Some(&tp),
                    &mut st_b,
                    &inp,
                );
                let mut st_f = TransmissionState { park, ..fresh(&tp) };
                let free = step(
                    TransmissionMode::Hybrid,
                    &fp_free,
                    Some(&tp),
                    &mut st_f,
                    &inp,
                );
                for i in 0..2 {
                    let (b, f) = (braked.next_speeds[i], free.next_speeds[i]);
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
/// a launch, a driving turn, and a pivot, in both regenerative modes — and, for the
/// codex-3 split, from an asymmetric rolling start with a hard steer command at gentle
/// throttle (`F_s ≫ F_p`, `m > d > 0`): the case where one SPROCKET's power is negative
/// while both MODAL powers read positive, so the modal split never charged η.
#[test]
fn energy_bound_no_free_energy() {
    let (fp, tp) = (lab_fp(), lab_tp());
    for (mode, throttle, steer, seed) in [
        (TransmissionMode::Hybrid, 1.0, 0.0, [0.0f32, 0.0]),
        (TransmissionMode::Hybrid, 0.7, 0.6, [0.0, 0.0]),
        (TransmissionMode::Hybrid, 0.0, 1.0, [0.0, 0.0]),
        // Codex-3: steer-dominant at a rolling start — inner sprocket goes negative.
        (TransmissionMode::Hybrid, 0.2, 1.0, [4.0, 2.0]),
        (TransmissionMode::Hybrid, 0.2, -1.0, [4.0, 2.0]),
        (TransmissionMode::FixedRadii, 1.0, 0.0, [0.0, 0.0]),
        (TransmissionMode::FixedRadii, 0.7, 0.8, [0.0, 0.0]),
        (TransmissionMode::FixedRadii, 0.0, 1.0, [0.0, 0.0]),
        (TransmissionMode::FixedRadii, 0.2, 1.0, [4.0, 2.0]),
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
                let rep = step(mode, &fp, Some(&tp), &mut st, &inp);
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
                "{mode:?} t={throttle} s={steer} window {window}: delivered {delivered:.0} J \
                     > available {available:.0} J + released {released:.0} J"
            );
        }
    }
}

/// Codex-3 pin: the recirculation split reads the PHYSICAL sprocket powers, not the
/// modal ones. Steer-only at an asymmetric rolling start (`F_p = 0`, saturated `F_s`,
/// `v_L = 5, v_R = 3`): the outer sprocket delivers `F_s/2·v_L`, the inner ABSORBS
/// `F_s/2·v_R` — physical net `= F_s/2·(v_L − η·v_R)`, while the modal split reads
/// `F_s·d` with no negative term at all. The reported power_scale must be the physical
/// one (and measurably NOT the modal one). Stage B: the scenario runs inside a shift
/// window (`shift_ticks: 5`, declutched) so the engine path contributes NO m-axis
/// force — what is pinned here is the SPLIT LAW, isolated from the crank coupling
/// (engaged, the cold crank against a 4 m/s shaft would add a clutch transient that
/// obscures the arithmetic).
#[test]
fn recirculation_splits_physical_output_powers() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = TransmissionState {
        gear: 5,
        shift_ticks: 5,
        ..fresh(&tp)
    };
    let (vl, vr) = (5.0f32, 3.0);
    let rep = step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(0.0, 1.0, [vl, vr], [0.0, 0.0]),
    );
    // Saturated servo (target far past the band): F_s = 2 × per-output capacity.
    let f_s = 2.0 * tp.steer_capacity_n;
    let (p_l, p_r) = (f_s / 2.0 * vl, -f_s / 2.0 * vr);
    let physical_net = p_l - tp.recirculation * -p_r;
    let expect = rep.power_available / physical_net;
    assert!(
        (rep.power_scale - expect).abs() < 1e-3,
        "power_scale {} must be the physical-output split {expect}",
        rep.power_scale
    );
    let modal = rep.power_available / (f_s * ((vl - vr) / 2.0));
    assert!(
        (rep.power_scale - modal).abs() > 0.02,
        "the physical split must be distinguishable from the modal one here \
             (physical {expect} vs modal {modal}) — otherwise this test pins nothing"
    );
}

/// The codex-1 regression (the "cannot decelerate" bug): a forward-moving tank given
/// full REVERSE throttle must brake monotonically to near standstill (service brakes),
/// then engage the reverse ladder at the swap seam and actually drive backward — the
/// old code fed `dir × |throttle|` through the still-forward ladder, producing full
/// FORWARD force and releasing engine drag: opposite input accelerated the tank.
#[test]
fn opposite_throttle_at_speed_brakes_then_reverses() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = TransmissionState {
        gear: 4,
        ..fresh(&tp)
    };
    let mut speeds = [6.0f32, 6.0];
    let mut m = 6.0f32;
    let mut swapped_at = None;
    for tick in 0..1024 {
        // Reactions zero — the hardest case: the OLD code accelerated forward here.
        let inp = input(-1.0, 0.0, speeds, [0.0, 0.0]);
        let rep = step(TransmissionMode::Hybrid, &fp, Some(&tp), &mut st, &inp);
        let m_next = (rep.next_speeds[0] + rep.next_speeds[1]) / 2.0;
        if swapped_at.is_none() {
            assert!(
                m_next <= m + 1e-4,
                "tick {tick}: opposite throttle must never accelerate forward \
                     (m {m} -> {m_next})"
            );
        }
        if st.reverse && swapped_at.is_none() {
            assert!(
                m.abs() < DIRECTION_SWAP_SPEED,
                "the reverse ladder must engage only near standstill (m = {m})"
            );
            swapped_at = Some(tick);
        }
        speeds = rep.next_speeds;
        m = m_next;
    }
    assert!(
        swapped_at.is_some(),
        "the held reverse command never engaged the reverse ladder (m = {m})"
    );
    assert!(
        m < -0.5,
        "after the swap the tank must actually drive backward (m = {m})"
    );
}

/// Coast intent (stage B shape): zero throttle at speed applies the DECLARED
/// compression-braking drag — `drag_fraction × peak torque` — at the CRANK, and it
/// reaches the belt only through the engaged coupling. With the belt speed HELD
/// constant (this harness feeds fixed speeds), the crank must be steady too, so the
/// clutch transmits the FULL drag torque: the converged per-side force is exactly the
/// old declared share `drag_fraction × peak × G/r_s / 2` (the steady state is
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
        rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, [v, v], [0.0, 0.0]),
        );
    }
    assert_eq!(st.gear, 3, "mid-band coast must not shift");
    let expect = -(tp.peak_torque_nm * tp.drag_fraction * g3 / tp.sprocket_radius) / 2.0;
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

/// The pivot-authority convention (the Tiger pivot-dead fix): the steering member
/// drives the two OUTPUTS differentially, so each output may carry up to the full
/// PER-OUTPUT capacity (`F_s` bounded by `2 × capacity`, `±capacity` per belt) — not
/// `±capacity/2`, which halves the yaw moment and left the Tiger under its own
/// footprint scrub. At rest under full steer the Hybrid commands full steer FORCE
/// outright (fix 2 — the power-limited pivot; the power scale cannot bind at v = 0),
/// and the L600's brake-gated neutral regime asks the semi-implicit servo for the
/// exact-landing force `2·neutral_d_full·I/dt`, capacity-clamped — for the lab data
/// both must land each side at the FULL per-output datum (which EXCEEDS the old
/// difference-axis reading's `capacity/2` ceiling outright).
#[test]
fn pivot_authority_is_per_output_capacity() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let dt = 1.0 / 64.0;
    for mode in [TransmissionMode::Hybrid, TransmissionMode::FixedRadii] {
        let mut st = fresh(&tp);
        let rep = step(
            mode,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 1.0, [0.0, 0.0], [0.0, 0.0]),
        );
        let expect = match mode {
            // Fix 2: force command up to capacity, power-limited thereafter.
            TransmissionMode::Hybrid => tp.steer_capacity_n,
            // The neutral servo's exact-landing force, per-output capacity clamp.
            _ => (tp.neutral_d_full * fp.inertia / dt).min(tp.steer_capacity_n),
        };
        assert!(
            (rep.forces[0] - expect).abs() < 1.0,
            "{mode:?}: left output must carry min(capacity, exact-landing) = {expect}, \
                 got {}",
            rep.forces[0]
        );
        assert!(
            (rep.forces[1] + expect).abs() < 1.0,
            "{mode:?}: right output mirrors it (counter-rotation), got {}",
            rep.forces[1]
        );
        assert!(
            expect > 0.9 * tp.steer_capacity_n,
            "the lab targets must exercise near-capacity authority ({expect} vs \
                 {}) — under the old difference-axis reading the ceiling was capacity/2",
            tp.steer_capacity_n
        );
    }
}

/// The gearing-implied top speed: the lab ladder's top gear at governed rpm is the
/// authored 52.2 km/h × (governed/rated) — the value the sandbox straight-line gate
/// asserts the measured speed against.
#[test]
fn geared_top_speed_matches_authoring() {
    let tp = lab_tp();
    let expect = 52.2 / 3.6 * (1800.0 / 1800.0);
    assert!((tp.geared_top_speed() - expect).abs() < 0.01);
}

/// Stage B: a standing start under full W is CLUTCH-SLIP-LIMITED. From rest the lock
/// torque `τ_c*` (lab arithmetic: `[ω_idle/dt + τ_free/J]/(1/J + k₁²/I_m)` =
/// `[62.8·64 + 1650/4]/[0.25 + 84.8²·(1/16000)]` ≈ 6.3 kN·m) far exceeds the 2860 N·m
/// clutch capacity, so the belt force is `k₁ × capacity` ≈ 242.5 kN — NOT the old
/// rev-floor peak-torque value `peak × G₁/r_s` ≈ 186.6 kN. The crank must never dip
/// below the stall-guard floor while the clutch slips (the saturated idle governor
/// holds a sub-idle slip equilibrium ≈ 37 rpm of droop where `τ_ind + τ_idle` meets
/// the capacity).
#[test]
fn launch_is_clutch_slip_limited() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let k1 = tp.gears_fwd[0] / tp.sprocket_radius;
    let expect = k1 * tp.clutch_capacity;
    let old_rev_floor = tp.peak_torque_nm * tp.gears_fwd[0] / tp.sprocket_radius;
    let floor = (tp.engine.idle_rpm - STALL_GUARD_BAND_RPM) * RPM_TO_RAD;
    let mut st = fresh(&tp);
    for tick in 0..16 {
        let rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [0.0, 0.0], [0.0, 0.0]),
        );
        let total = rep.forces[0] + rep.forces[1];
        assert!(
            (total - expect).abs() < 0.01 * expect,
            "tick {tick}: launch belt force {total:.0} N must be clutch-capacity \
                 limited ({expect:.0} N)"
        );
        assert!(
            (total - old_rev_floor).abs() > 0.1 * old_rev_floor,
            "the capacity-limited launch must be measurably NOT the old rev-floor \
                 value ({old_rev_floor:.0} N) — otherwise this test pins nothing"
        );
        assert!(
            st.omega_e >= floor - 1e-3,
            "tick {tick}: the slipping-clutch launch must never stall the crank \
                 below idle − band ({:.0} rpm)",
            st.omega_e / RPM_TO_RAD
        );
    }
}

/// Stage B: the stall guard under a grade lug — the crank NEVER lands below
/// idle − [`STALL_GUARD_BAND_RPM`], in both slip regimes: (a) full-W lug against an
/// impossible reaction (capacity-clamped slip: the sub-idle equilibrium sits where the
/// saturated idle governor + low-end torque meet the 2860 N·m capacity, ≈ 37 rpm of
/// droop — above the 100 rpm guard band); (b) a zero-throttle engaged backslide
/// (τ_c* wants the crank at the NEGATIVE shaft speed — the guard slips the clutch
/// instead and the belt receives the crank's forward τ_free through it).
#[test]
fn stall_guard_holds_crank_under_grade_lug() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let floor = (tp.engine.idle_rpm - STALL_GUARD_BAND_RPM) * RPM_TO_RAD;
    for (throttle, speeds, reactions, label) in [
        (
            1.0f32,
            [0.0f32, 0.0],
            [200_000.0f32, 200_000.0],
            "full-W lug",
        ),
        (0.0, [-2.0, -2.0], [-40_000.0, -40_000.0], "coast backslide"),
    ] {
        let mut st = fresh(&tp);
        for tick in 0..128 {
            let rep = step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(throttle, 0.0, speeds, reactions),
            );
            assert!(
                st.omega_e >= floor - 1e-3,
                "{label} tick {tick}: ω_e {:.0} rpm fell below the stall-guard floor \
                     ({:.0} rpm)",
                st.omega_e / RPM_TO_RAD,
                floor / RPM_TO_RAD
            );
            assert!(
                rep.forces[0] > 0.0 && rep.forces[1] > 0.0,
                "{label} tick {tick}: the slipping clutch must keep delivering \
                     FORWARD drive (forces {:?})",
                rep.forces
            );
        }
    }
}

/// Stage B: rev-match across an upshift — the crank is CONTINUOUS through the window
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
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [v_warm, v_warm], [0.0, 0.0]),
        );
    }
    let target_rpm = v_up * g2 / tp.sprocket_radius / RPM_TO_RAD; // ≈ 1121
    let mut prev_rpm = st.omega_e / RPM_TO_RAD;
    let mut window_end_gap = None;
    let mut ticks_since_window = 0u32;
    let mut rep = TransmissionReport::default();
    for tick in 0..96 {
        rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
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

/// Stage B: unloaded free-rev — declutched full steer at standstill (the pivot's crank
/// demand) revs the crank from idle toward the steer-demand target (the PEAK-TORQUE
/// rpm — the old floor's operating point, reached dynamically; deliberately NOT the
/// governed cut-out, where `torque_at·ω = 0` would zero the pivot's power budget).
/// Lab arithmetic: Δω = 500 rpm = 52.4 rad/s at ≈ τ/J ≈ 2000/4 = 500 rad/s² plus the
/// proportional-band taper → ≈ 0.15–0.3 s to 95%; the steady point parks ≈ 30 rpm
/// under the target where the taper's fueling meets the re-engaging drag. Pinned with
/// margin.
#[test]
fn free_rev_reaches_steer_target_promptly() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    let mut reached = None;
    for tick in 0..128 {
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 1.0, [0.0, 0.0], [0.0, 0.0]),
        );
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

/// Review round FIX 1: service braking must never TELEPORT the crank through an
/// infeasible snap. The eager `exact` flag was decided at the pre-brake coupling
/// solve; the brake stop-force then dropped the belt ≈ 0.23 m/s per tick
/// (120 kN / 8 t / 64 Hz) and the drift kill snapped the crank down with it —
/// ≈ 20 rad/s per tick, an implied clutch torque
/// `τ_impl = τ_free − Δω·J/dt ≈ −550 − 20·4·64 ≈ −5.7 kN·m` through a 2.86 kN·m
/// clutch. Post-fix the snap is feasibility-checked on the FINAL belt state: the crank
/// integrates honestly with the torque the clutch actually carried, so its per-tick
/// change obeys `|Δω|·J/dt ≤ capacity + |τ_free|` (braking: |τ_free| = drag ≤
/// drag_fraction × peak = 550 N·m → |Δω| ≤ (2860 + 550)·dt/J ≈ 13.3 rad/s).
#[test]
fn braking_never_teleports_crank_past_clutch_capacity() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let dt = 1.0 / 64.0;
    let mut st = fresh(&tp);
    // Warm to a locked coast in gear 1 at m = 1.0 (held speeds).
    for _ in 0..32 {
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, [1.0, 1.0], [0.0, 0.0]),
        );
    }
    assert!(
        st.omega_e / RPM_TO_RAD > tp.engine.idle_rpm,
        "the warm-up must have locked the crank above idle"
    );
    // Full opposing throttle, CLOSED loop: the brake-driven belt drop must not drag
    // the crank faster than the clutch can physically pull it.
    let drag_max = tp.peak_torque_nm * tp.drag_fraction;
    let slew_bound = (tp.clutch_capacity + drag_max) * dt / tp.engine_inertia + 0.1;
    let mut speeds = [1.0f32, 1.0];
    for tick in 0..64 {
        let m_pre = (speeds[0] + speeds[1]) / 2.0;
        if m_pre < 0.5 {
            break; // swap/declutch territory — the teleport window is over.
        }
        let omega_pre = st.omega_e;
        let rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(-1.0, 0.0, speeds, [0.0, 0.0]),
        );
        let delta = st.omega_e - omega_pre;
        assert!(
            delta >= -slew_bound,
            "tick {tick}: braking dragged the crank {delta:.1} rad/s in one tick — \
                 an implied clutch torque past capacity (bound {slew_bound:.1} rad/s); \
                 the infeasible snap is back"
        );
        speeds = rep.next_speeds;
    }
    let floor = (tp.engine.idle_rpm - STALL_GUARD_BAND_RPM) * RPM_TO_RAD;
    assert!(
        st.omega_e >= floor,
        "the crank must end above the hard floor"
    );
}

/// Review round FIX 3: the coupling seam is a LATCH with hysteresis — a belt speed
/// oscillating INSIDE the 0.4–0.6 m/s dead band (forced ±0.05 around the old single
/// 0.5 threshold, which flipped the regime every crossing) produces ZERO regime
/// flips; only genuine excursions past the separated thresholds transition it, once
/// each. Scripted open-loop: park below 0.4 (one transition out), 64 boundary
/// oscillations (none), one push past 0.6 (one transition in), 64 more oscillations
/// (none).
#[test]
fn clutch_seam_hysteresis_kills_boundary_chatter() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    let at = |st: &mut TransmissionState, v: f32| {
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            st,
            &input(0.0, 0.0, [v, v], [0.0, 0.0]),
        );
        st.clutch_out
    };
    // Park below CLUTCH_OUT_M_SPEED: the clutch goes out.
    assert!(at(&mut st, 0.3), "below 0.4 m/s the clutch must go out");
    // Boundary oscillation across the OLD single threshold: no flips.
    for tick in 0..64 {
        let v = if tick % 2 == 0 { 0.55 } else { 0.45 };
        assert!(
            at(&mut st, v),
            "tick {tick}: an in-band oscillation (0.45/0.55) must not re-engage — \
                 the single-threshold chatter is back"
        );
    }
    // A genuine excursion past CLUTCH_IN_M_SPEED re-engages…
    assert!(!at(&mut st, 0.65), "past 0.6 m/s the clutch must re-engage");
    // …and the same boundary oscillation now holds ENGAGED: no flips either way.
    for tick in 0..64 {
        let v = if tick % 2 == 0 { 0.55 } else { 0.45 };
        assert!(
            !at(&mut st, v),
            "tick {tick}: an in-band oscillation must not declutch after engagement"
        );
    }
}
