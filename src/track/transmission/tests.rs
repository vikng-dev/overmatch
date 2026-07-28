use super::*;

/// A lab ForceParams: only the fields the transmission reads matter (inertia, max_speed,
/// slip_saturation, grip_stiffness envelope switch, and the governor's own knobs).
fn lab_fp() -> ForceParams {
    ForceParams {
        face_offset: 0.02,
        free_travel: 0.0,
        support_stiffness_per_m: 680_000.0,
        support_damping_per_m: 80_000.0,
        engage_depth: 0.02,
        probe_reach: 0.5,
        mu: 0.9,
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
/// Down band 900: the load-time validator now includes the runtime landing margin
/// (`shift_up × min_step > shift_down + POSTSHIFT_MARGIN_RPM`), and the lab ladder's
/// tightest step (12.7/20.4 ≈ 0.6225) lands 1700 × 0.6225 ≈ 1058 — the old 950 band
/// (floor 1100) was itself an illegal triple that only ever shifted by over-running.
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
        shift_down_rpm: 900.0,
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

/// The shipped Tiger's declared drivetrain authoring (mirrors tiger_1.tank.ron — down band
/// 1150 with the RON's rationale), kept local to arithmetic tests so they exercise the same
/// authored curve and speed anchors without reaching through the ECS/spec adapter. Exposed
/// as the authoring struct so validator tests can mutate one field and re-validate.
fn tiger_authoring() -> TransmissionAuthoring<'static> {
    TransmissionAuthoring {
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
        shift_down_rpm: 1150.0,
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
    }
}

fn tiger_tp() -> TransmissionParams {
    TransmissionParams::from_authoring(&tiger_authoring())
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

/// [`fresh`] with the upshift confirmation PRE-PAID: most arithmetic fixtures pin OTHER gates at an operating
/// point already above the up band, so they seed [`UPSHIFT_CONFIRM_TICKS`] worth of
/// evidence rather than re-testing the confirmation dwell in every fixture (the committing
/// tick still re-proves every gate live — the seeded counter only skips the wait). Tests
/// OF the confirmation construct from `fresh` and pay it live
/// (`upshift_confirmation_rejects_transient_band_excursions`).
fn confirmed(tp: &TransmissionParams) -> TransmissionState {
    TransmissionState {
        band_confirm_ticks: UPSHIFT_CONFIRM_TICKS,
        ..fresh(tp)
    }
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
    const REPLICATE_EXACT_FIELDS: usize = 17;
    const DERIVE_FIELDS: usize = 0;
    const LOCAL_VIEW_FIELDS: usize = 0;

    // Adding a field? Classify it in transmission-design.md's authoritative REV-14 inventory,
    // then extend the exhaustive canonical projection. Do not add `..` there.
    let classified_fields = transmission_state_projection(&fresh(&lab_tp()));
    assert_eq!(classified_fields[0].name, "gear");
    assert_eq!(classified_fields[12].name, "band_confirm_ticks");
    assert_eq!(classified_fields[16].name, "hold_reengage_ticks");

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
        ..confirmed(&tp)
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
        ..confirmed(&tp)
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
/// shape as the addressing test is presented with the 20-tick DERIVED window under a reaction
/// strong enough to arrest the belt inside the cut EVEN under the honest decayed-reaction /
/// coupled-mass predictor (470 kN/side: Δm ≈ 470 000 × 4.94 ticks / 64 Hz / 29 250 kg ≈
/// 1.24 m/s > the 1.16 m/s operating point) — `landing_m < 0`, so no grade shift may commit.
/// (The old frozen predictor read a mere 20° grade reaction as a backward landing; the honest
/// model rightly does not — declutching at 1.16 m/s on 20° still lands forward — so the
/// mechanism pin needs a genuinely arresting reaction.)
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
        &input(1.0, 0.0, [shaft; 2], [470_000.0; 2]),
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

/// D1c regression: a successful handoff suppresses relatching for the
/// FULL fixed cooldown — never overridable (a rollback override would let a
/// force-based release re-latch the very next tick while cross-moving; mid-motion braking
/// is `back_driven_intent`'s job, and the latch itself is near-rest-only) — and once the
/// cooldown expires a still-standing deficit relatches normally.
#[test]
fn hill_hold_release_cooldown_never_overridden_then_relatches() {
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

    // A roll past the engagement threshold does NOT override the cooldown —
    // and could not latch anyway (the zone is near-rest-only); `back_driven_intent`
    // braking owns the moving hull.
    st.demand_n = deficit_demand;
    let rollback = -(HILL_HOLD_ENGAGE_SPEED + 0.01);
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [rollback; 2], [deficit_demand / 2.0; 2]),
    );
    assert!(
        !st.hill_hold,
        "no relatch during the cooldown, whatever the motion"
    );

    // Drain the remaining cooldown at rest; the standing deficit then relatches.
    for _ in 0..HOLD_REENGAGE_TICKS {
        st.demand_n = deficit_demand;
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [0.0; 2], [deficit_demand / 2.0; 2]),
        );
    }
    assert!(
        st.hill_hold,
        "a deficit still standing at cooldown expiry must relatch"
    );
}

/// The hill-hold latch engages ONLY near rest. All four intent-vs-motion
/// quadrants AT SPEED (|shaft| = 5 m/s ≫ the engagement threshold) must not latch — the
/// counterexample was held reverse throttle with the hull still moving forward
/// (propulsive intent, shaft = −5, the old rollback arm latched a 5 m/s "rollback"); the
/// cross-motion quadrants are `back_driven_intent` braking territory instead. The same
/// deficit AT REST latches on both ladders (the legitimate engagements).
#[test]
fn hill_hold_engages_only_near_rest() {
    let mut tp = tiger_tp();
    // A real (2-tick) window keeps the latch observable: with a zero-length window a
    // capable launch gear latches, transmits, and releases inside the same tick.
    tp.shift_ticks = 2;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
    let run = |reverse: bool, throttle: f32, m: f32| -> TransmissionState {
        let gear = if reverse { 4 } else { 5 };
        let mut st = TransmissionState {
            gear,
            reverse,
            demand_n: demand,
            demand_initialized: true,
            ..fresh(&tp)
        };
        // Reactions carry the demand's ladder-signed projection; their exact value is
        // irrelevant to the latch decision under test.
        let dir = if reverse { -1.0 } else { 1.0 };
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(throttle, 0.0, [m; 2], [dir * demand / 2.0; 2]),
        );
        st
    };

    for (reverse, throttle, m, label) in [
        (false, 1.0, 5.0, "F ladder, W, moving forward"),
        (
            false,
            1.0,
            -5.0,
            "F ladder, W, rolling backward (cross-motion)",
        ),
        (true, -1.0, -5.0, "R ladder, S, moving backward"),
        (
            true,
            -1.0,
            5.0,
            "R ladder, S, rolling forward (cross-motion)",
        ),
    ] {
        let st = run(reverse, throttle, m);
        assert!(
            !st.hill_hold,
            "{label}: the latch must never engage at speed"
        );
    }

    for (reverse, throttle, label) in [
        (false, 1.0, "F ladder at rest"),
        (true, -1.0, "R ladder at rest"),
    ] {
        let st = run(reverse, throttle, 0.0);
        assert!(st.hill_hold, "{label}: the near-rest deficit must latch");
    }
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
/// otherwise-valid above-band upshift preference on the same decision tick. This is a REAL
/// two-leg collision proof. The
/// arbitration is STRUCTURAL — the deficit path runs before `upshift_ready` is even
/// evaluated, its commit zeroes `band_confirm_ticks`, and `deficit_step_committed`
/// falsifies the predicate — so a seeded upshift counter alone proves nothing about the
/// main leg (the counter never reaches N on the collision tick). The CONTROL leg is what
/// makes the coverage honest: the identical state with the deficit confirmation ONE TICK
/// FARTHER from crossing commits the F2→F3 upshift on that very tick, proving the
/// upshift arm (band + landing + reserve + counter at N − 1, all asserted below) was
/// genuinely one tick from firing in the main leg — where the deficit crosses on the
/// same tick and must win with F1. This is the coverage
/// `grade_confirmation_paces_commit_when_target_already_legal` defers to for its lifted
/// band.
#[test]
fn confirmed_deficit_precedes_upshift_arm() {
    let (fp, tp, shaft, demand) = correction_priority_fixture();
    // The upshift arm must be one tick from ready ON ITS OWN TERMS, or this test pins
    // nothing: prove band, landing, and reserve for the F2→F3 upshift all pass.
    assert!(
        shaft * tp.gears_fwd[1] / tp.sprocket_radius / RPM_TO_RAD > tp.shift_up_rpm,
        "fixture must sit above the up band (the band leg of the upshift predicate)"
    );
    let g_up = tp.gears_fwd[2];
    let landing = predict_shift_landing_m(&tp, &fp, shaft, demand / 2.0, 1.0 / 64.0);
    assert!(
        landing > 0.0
            && landing * g_up / tp.sprocket_radius / RPM_TO_RAD
                >= tp.shift_down_rpm + POSTSHIFT_MARGIN_RPM,
        "fixture's predicted F3 landing must clear the fix-1a gate (the landing leg)"
    );
    assert!(
        modeled_reserve_in_gear(&tp, &fp, shaft, g_up, demand) >= reserve_margin(demand),
        "fixture's F3 reserve must clear the margin (the reserve leg)"
    );
    let contender = |grade_confirm_ticks: u8| TransmissionState {
        gear: 2,
        demand_n: demand,
        demand_initialized: true,
        grade_confirm_ticks,
        // One tick from an armed upshift: the counter crosses N on the stepped tick.
        band_confirm_ticks: UPSHIFT_CONFIRM_TICKS - 1,
        ..fresh(&tp)
    };
    let inp = input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]);

    // CONTROL leg: the deficit confirmation sits one tick farther from crossing, so the
    // deficit does NOT confirm on this tick — and the very same upshift-arm state must
    // then commit F2→F3. This proves the upshift was genuinely one step from firing;
    // without it, the main leg's F1 would pass even with the arbitration broken.
    let mut control = contender(GRADE_CONFIRM_TICKS - 2);
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut control,
        &inp,
    );
    assert_eq!(
        control.gear, 3,
        "control: with the deficit unconfirmed on this tick, the armed upshift must \
             commit F2->F3 — otherwise the main leg's collision is fictional"
    );

    // MAIN leg: identical state, but the deficit confirmation crosses on the SAME tick
    // as the upshift's — the deficit owns the tick and corrects downward.
    let mut st = contender(GRADE_CONFIRM_TICKS - 1);
    step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
    assert_eq!(
        st.gear, 1,
        "the confirmed deficit must correct DOWNWARD through the simultaneously-armed \
             upshift (an F3 here means the upshift arm won the tick)"
    );
}

/// D5 (the upshift arm is LIVE in this fixture — no masking): a SHALLOW confirmed deficit (inside the reserve-margin
/// scale, the demand-pollution class) is DEFERRED by the post-upshift reversal dwell while
/// the pending correction HOLDS the gear — the operating point sits above the band
/// (1800 > 1700) with a fully capable upper gear, so without the hold suppression the
/// ordinary arm would commit F2→F3 on the first tick, resetting the very confirmation
/// evidence and dwell that were deferring the correction (evidence erased, correction
/// never fires). The evidence keeps ACCUMULATING through the dwell, so
/// the correction commits exactly at dwell expiry: F2 held throughout, then F1, and F3
/// never. A post-DOWNSHIFT dwell never blocks the correction (same-direction;
/// `dwell_blocks` is direction-aware) — `confirmed_deficit_precedes_upshift_arm` covers
/// the undwelled tick.
#[test]
fn confirmed_deficit_defers_through_post_upshift_dwell() {
    let (fp, tp, shaft, _) = correction_priority_fixture();
    // A SHALLOW deficit: 4 kN below zero reserve, well inside the margin scale
    // (0.1·demand + 10 kN), so the deferral — not the deep override — owns it.
    let current = available_force_in_gear(&tp, &fp, shaft, tp.gears_fwd[1]);
    let demand = current + 4_000.0;
    assert!(
        demand - current < reserve_margin(demand),
        "fixture must be a SHALLOW deficit (depth {} vs margin {})",
        demand - current,
        reserve_margin(demand)
    );
    assert!(
        shaft * tp.gears_fwd[1] / tp.sprocket_radius / RPM_TO_RAD > tp.shift_up_rpm,
        "fixture must keep the ordinary upshift arm live above the band"
    );
    assert!(
        available_force_in_gear(&tp, &fp, shaft, tp.gears_fwd[2]) - demand
            >= reserve_margin(demand),
        "fixture's upper gear must be capable — the upshift is refused by the HOLD, \
             not by its own reserve gate"
    );
    let mut st = TransmissionState {
        gear: 2,
        last_shift_dir: 1,
        dwell_ticks: REVERSAL_DWELL_TICKS,
        demand_n: demand,
        demand_initialized: true,
        grade_confirm_ticks: GRADE_CONFIRM_TICKS - 1,
        ..fresh(&tp)
    };
    let inp = input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]);
    for tick in 0..REVERSAL_DWELL_TICKS {
        step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
        assert_eq!(
            st.gear, 2,
            "tick {tick}: the pending correction must HOLD the gear through the \
                 post-upshift dwell — neither upshifted over (F3) nor corrected early \
                 (F1); dwell {}",
            st.dwell_ticks
        );
    }
    assert_eq!(st.dwell_ticks, 0, "the dwell must have drained");
    step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
    assert_eq!(
        st.gear, 1,
        "the accumulated correction must commit at dwell expiry"
    );
}

/// Near-arrest safety (the deep override): a deficit DEEPER than the
/// reserve-margin scale is a genuine steep grade, not post-shift demand pollution — it
/// corrects IMMEDIATELY through the post-upshift dwell instead of deferring, before
/// window + dwell can bleed the predicted landing sign negative and strand the correction.
#[test]
fn deep_deficit_overrides_post_upshift_deferral() {
    let (fp, tp, shaft, _) = correction_priority_fixture();
    let current = available_force_in_gear(&tp, &fp, shaft, tp.gears_fwd[1]);
    let demand = current + 30_000.0;
    assert!(
        demand - current > reserve_margin(demand),
        "fixture must be a DEEP deficit (depth {} vs margin {})",
        demand - current,
        reserve_margin(demand)
    );
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
        "a deep deficit must override the post-upshift deferral immediately"
    );
}

/// Closed-loop pin: a steep grade entered RIGHT AFTER an upshift —
/// post-window state, reversal dwell fully live, confirmation nearly paid — must correct
/// downward on its first confirmed tick via the deep override, while the predicted landing
/// is still forward, and land moving forward after the paid window. Without the override
/// the deferral held the correction through the dwell while the grade bled speed: the
/// landing sign crossed, the correction became uncommittable, the ordinary downshift
/// refused on its `reserve >= 0` gate, and the vehicle lugged to the near-rest hill-hold
/// seam in the tall gear.
#[test]
fn steep_grade_after_upshift_corrects_before_lug() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 2;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    fp.grip_stiffness = forces::grip_stiffness(0.9, 57_000.0 * 9.81);
    // The slope investigation's F4 operating point meeting a DERIVED 25-degree demand.
    let demand = 57_000.0 * 9.81 * 25.0_f32.to_radians().sin();
    let f4 = tp.gears_fwd[3];
    let shaft0 = 980.0 * RPM_TO_RAD * tp.sprocket_radius / f4;
    assert!(
        demand - available_force_in_gear(&tp, &fp, shaft0, f4) > reserve_margin(demand),
        "fixture must be a deep deficit for F4"
    );
    let mut st = TransmissionState {
        gear: 4,
        last_shift_dir: 1,
        dwell_ticks: REVERSAL_DWELL_TICKS,
        omega_e: 980.0 * RPM_TO_RAD,
        demand_n: demand,
        demand_initialized: true,
        grade_confirm_ticks: GRADE_CONFIRM_TICKS - 1,
        ..fresh(&tp)
    };
    let mut speeds = [shaft0; 2];
    let first = step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, speeds, [demand / 2.0; 2]),
    );
    speeds = first.next_speeds;
    assert!(
        st.gear < 4 && matches!(st.scheduler, SchedulerState::GradeShift { from: 4, .. }),
        "the deep deficit must grade-correct on its FIRST confirmed tick, dwell or not \
             (got F{}, {:?})",
        st.gear,
        st.scheduler
    );
    // Ride out the paid window and a few engaged ticks: the corrected drivetrain must stay
    // moving FORWARD (the lug bled through zero in the tall gear before the override).
    for _ in 0..8 {
        let rep = step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, speeds, [demand / 2.0; 2]),
        );
        speeds = rep.next_speeds;
    }
    let m = (speeds[0] + speeds[1]) / 2.0;
    assert!(
        m > 0.0,
        "the corrected vehicle must keep moving FORWARD after the paid window \
             (m = {m:.3}), not lug through zero in the tall gear"
    );
    assert!(st.gear < 4, "the correction must stand (got F{})", st.gear);
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

/// D6 regression: the protective upshift is a LAST RESORT at the
/// mechanical-protection ceiling (`max_curve_rpm` — the Tiger's rated 3000). A signed
/// shaft + crank back-driven PAST the ceiling upshifts with the throttle released, and the
/// declutched crank slows. Everything between governed and the ceiling HOLDS instead —
/// see `overrun_below_ceiling_holds_gear_for_engine_braking`.
#[test]
fn downhill_overrun_protective_upshift_lowers_crank_speed() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 2;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let overrun_rpm = tp.max_curve_rpm() + 100.0;
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

/// On overrun BETWEEN governed and the mechanical-protection
/// ceiling the box HOLDS its gear — engine braking is the point of being in gear on a
/// descent, and a governed + margin protective upshift would shed exactly that
/// retardation. Coasting AND service-braking, with the crank fully corroborating the
/// shaft (the honest back-driven case, not a belt transient): no shift commits and no
/// window starts. The dial climbing past governed is the warning; only past
/// `max_curve_rpm` does the last resort fire (test above).
#[test]
fn overrun_below_ceiling_holds_gear_for_engine_braking() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 2;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    // Deep past the old governed + 150 firing floor, still under the curve-top ceiling.
    let overrun_rpm = (tp.engine.governed_rpm + tp.max_curve_rpm()) / 2.0;
    assert!(
        overrun_rpm > tp.engine.governed_rpm + CRANK_CORROBORATION_MARGIN_RPM
            && overrun_rpm < tp.max_curve_rpm(),
        "fixture must sit between the old floor and the ceiling"
    );
    let g4 = tp.gears_fwd[3];
    let shaft = overrun_rpm * RPM_TO_RAD * tp.sprocket_radius / g4;
    for throttle in [0.0, -1.0] {
        let mut st = TransmissionState {
            gear: 4,
            omega_e: overrun_rpm * RPM_TO_RAD,
            demand_initialized: true,
            ..fresh(&tp)
        };
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(throttle, 0.0, [shaft; 2], [-40_000.0; 2]),
        );
        assert_eq!(
            st.gear, 4,
            "throttle {throttle}: overrun under the ceiling must HOLD the gear"
        );
        assert_eq!(
            st.shift_ticks, 0,
            "throttle {throttle}: no interruption window may start"
        );
    }
}

/// Best-effort selection, guard-bounded: the ceiling rescue on
/// the Tiger's 4-gear REVERSE ladder under sustained hill drive, CLOSED LOOP. Selection
/// is the STATIC projection only (current shaft through the candidate ratio); the
/// over-rev slip guard — not the selection — is the safety mechanism, so the honest
/// contract is behavioral: the crank never exceeds the guard point on ANY tick, the gear
/// walk is monotone upward as the hill keeps accelerating the belt, the ladder tops out
/// at R4, and the crank ends pinned at/below the guard. Hill-driving assistance on the
/// reverse ladder is POSITIVE reaction (`v += (q − r)·dt/I`). Also: an overspeed so
/// extreme that even the TOP gear's static projection exceeds the ceiling HOLDS (a
/// commit would only shed reflected braking while the guard already owns the crank), and
/// a top-gear overspeed has nothing above — holds.
#[test]
fn protective_rescue_walks_ladder_under_guard_or_holds() {
    let tp = tiger_tp();
    assert_eq!(
        tp.shift_ticks, 20,
        "the authored window must stay realistic"
    );
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let ceiling = tp.max_curve_rpm();
    let guard = (ceiling + OVERREV_MARGIN_RPM) * RPM_TO_RAD;
    let m_of = |gear: u8, rpm: f32| {
        -(rpm * RPM_TO_RAD * tp.sprocket_radius / tp.gears_rev[(gear - 1) as usize])
    };
    let state = |gear: u8, rpm: f32| TransmissionState {
        gear,
        reverse: true,
        omega_e: rpm * RPM_TO_RAD,
        demand_initialized: true,
        ..fresh(&tp)
    };

    // Closed-loop walk: R3 just past the ceiling under a hill that out-pulls even R4's
    // guard-point drag (per side 60 kN > 47.4 kN — τ_drag(3100)·k(R4)/2), so the overrun
    // is genuinely sustained. Low-gear coast overruns are SELF-ARRESTING instead: R1/R2
    // reflect hundreds of kN of motoring drag at the ceiling, more than any plausible
    // hill, so a coast walk from the bottom of the ladder decelerates and band-downshifts
    // — the probes' full reverse walks are S-HELD (fueled), not coasts.
    let start_rpm = ceiling + 50.0;
    let mut st = state(3, start_rpm);
    let mut speeds = [m_of(3, start_rpm); 2];
    let mut prev_gear = st.gear;
    for tick in 0..256 {
        let rep = step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, speeds, [60_000.0; 2]),
        );
        speeds = rep.next_speeds;
        assert!(
            st.omega_e <= guard + 1e-3,
            "tick {tick}: crank {:.0} rpm past the guard point — the guard, THE safety \
                 mechanism, failed",
            st.omega_e / RPM_TO_RAD
        );
        assert!(
            st.gear >= prev_gear,
            "tick {tick}: the rescue walk must be monotone upward"
        );
        prev_gear = st.gear;
    }
    assert_eq!(
        st.gear, 4,
        "the sustained hill drive must walk the rescue to the top gear"
    );
    let shaft_end = -(speeds[0] + speeds[1]) / 2.0;
    assert!(
        shaft_end * tp.gears_rev[3] / tp.sprocket_radius / RPM_TO_RAD > ceiling,
        "the fixture must actually sustain the overrun in top gear or it proves nothing"
    );
    assert!(
        st.omega_e <= guard + 1e-3,
        "top gear under an unrelenting hill: the guard owns the crank ({:.0} rpm)",
        st.omega_e / RPM_TO_RAD
    );

    // Extreme overspeed: even R4's static projection exceeds the ceiling — hold.
    let mut hold = state(3, ceiling + 1_800.0);
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut hold,
        &input(0.0, 0.0, [m_of(3, ceiling + 1_800.0); 2], [40_000.0; 2]),
    );
    assert_eq!(
        hold.gear, 3,
        "no gear statically clears — hold; a commit would only shed reflected braking"
    );
    assert_eq!(hold.shift_ticks, 0, "no window may start on the hold");

    // Top gear: nothing above.
    let mut top = state(4, ceiling + 400.0);
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut top,
        &input(0.0, 0.0, [m_of(4, ceiling + 400.0); 2], [40_000.0; 2]),
    );
    assert_eq!(top.gear, 4, "top-gear overspeed has nothing above — holds");
    assert_eq!(top.shift_ticks, 0, "no window may start at the top");
}

/// A SEQUENTIAL box steps ONE gear per paid window toward the
/// static target, unconditionally — an adjacent-safe rule livelocks and an improvement
/// predicate would price a window model this path does not have; with the
/// slip guard bounding every intermediate window, stepping is always at least as good as
/// holding, and the strictly rising gear index on a finite ladder terminates trivially.
/// R2 far past the ceiling (static R3 ≈ 3120 is still over, R4 ≈ 2100 clears): the walk
/// must pay R2 → R3 → R4 one window at a time and reach R4; the same state Direct skips
/// straight to R4. The no-static-target hold is covered by the walk test above.
#[test]
fn sequential_rescue_steps_toward_static_target() {
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let overrun_rpm = tiger_tp().max_curve_rpm() + 1_500.0;

    let mut tp = tiger_tp();
    tp.shift_ticks = 2;
    tp.shift_addressing = ShiftAddressing::Sequential;
    let mut st = TransmissionState {
        gear: 2,
        reverse: true,
        omega_e: overrun_rpm * RPM_TO_RAD,
        demand_initialized: true,
        ..fresh(&tp)
    };
    let mut speeds = [-(overrun_rpm * RPM_TO_RAD * tp.sprocket_radius / tp.gears_rev[1]); 2];
    let mut prev_gear = st.gear;
    let mut reached_at = None;
    for tick in 0..16 {
        let rep = step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, speeds, [10_000.0; 2]),
        );
        speeds = rep.next_speeds;
        assert!(
            st.gear >= prev_gear,
            "tick {tick}: the walk must be monotone"
        );
        prev_gear = st.gear;
        if reached_at.is_none() && st.gear == 4 {
            reached_at = Some(tick);
        }
    }
    let reached = reached_at.expect("the Sequential walk must reach the static target R4");
    assert!(
        reached <= 2 * (tp.shift_ticks as usize + 1),
        "progress must be one gear per paid window, not eventual ({reached} ticks)"
    );

    let mut tp_direct = tiger_tp();
    tp_direct.shift_ticks = 2;
    let mut direct = TransmissionState {
        gear: 2,
        reverse: true,
        omega_e: overrun_rpm * RPM_TO_RAD,
        demand_initialized: true,
        ..fresh(&tp_direct)
    };
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp_direct),
        &mut direct,
        &input(
            0.0,
            0.0,
            [-(overrun_rpm * RPM_TO_RAD * tp_direct.sprocket_radius / tp_direct.gears_rev[1]); 2],
            [10_000.0; 2],
        ),
    );
    assert_eq!(
        direct.gear, 4,
        "Direct from the same state skips straight to the static target"
    );
}

/// The hill-hold latch's window-deficit arm is scoped by the
/// ABSOLUTE reserve floor — it fires only when the window-masked demand EMA exceeds
/// [`RESERVE_MARGIN_FLOOR_N`] (10 kN, the stage-C jitter/truth divide; owner-based
/// scoping was tried and broke the 20-degree Sequential band-cascade rescue the arm was
/// built for). Before the fix, ANY paid window with ANY positive demand EMA latched: an
/// ordinary tall-gear band downshift near rest on FLAT ground with a few kN of residual
/// demand exposed HILL HOLD out of nowhere.
#[test]
fn flat_downshift_window_does_not_latch_hill_hold() {
    let tp = tiger_tp();
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let mut st = TransmissionState {
        gear: 3,
        demand_n: 5_000.0,
        demand_initialized: true,
        ..fresh(&tp)
    };
    // Near rest under W on the flat: the band downshift chain commits immediately and
    // opens its paid window (capable — light demand, huge low-gear reserve).
    let inp = input(1.0, 0.0, [0.01; 2], [2_500.0; 2]);
    step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
    assert_eq!(st.gear, 2, "the ordinary band downshift must commit");
    assert!(st.shift_ticks > 0, "its window must be in flight");
    for tick in 0..tp.shift_ticks {
        step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
        assert!(
            !st.hill_hold,
            "tick {tick}: a flat-ground downshift window with residual demand must NOT \
                 latch hill hold"
        );
        assert_eq!(
            st.scheduler,
            SchedulerState::Normal,
            "tick {tick}: no grade status may appear on the flat"
        );
    }
}

/// The 13-tick grade confirmation ISOLATED — the target is
/// legal and margin-clear from tick one (`correction_priority_fixture`: zero-length
/// windows, so the landing predictor is a no-op and the over-rev gate passes), so the
/// ONLY pacing gate is the confirmation itself: twelve quiet ticks, commit on exactly
/// the thirteenth. The band arm's upshift confirmation is 8 <
/// 13 ticks, so at this synthetic operating point (above the band with a fixture-capable
/// UPPER gear — the contrived dead-zone torque curve) the now-faster ordinary arm would
/// legally commit F2→F3 on tick 8 and steal the crossing this test pins; the up band is
/// lifted out of the way so the DEFICIT pacing stays the mechanism under test (the
/// deficit-vs-live-upshift same-tick priority is pinned by
/// `confirmed_deficit_precedes_upshift_arm`).
#[test]
fn grade_confirmation_paces_commit_when_target_already_legal() {
    let (fp, mut tp, shaft, demand) = correction_priority_fixture();
    tp.shift_up_rpm = 10_000.0;
    let mut st = TransmissionState {
        gear: 2,
        demand_n: demand,
        demand_initialized: true,
        ..fresh(&tp)
    };
    let inp = input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]);
    // INDEPENDENT literal: 13 is pinned as a number, not derived from
    // GRADE_CONFIRM_TICKS — deriving both the loop bound and the expectation from the
    // constant meant changing the constant could never fail this test. 13 ties to the
    // constant's own doc: "13 ticks DERIVED = 0.203125 s at 64 Hz".
    const PINNED_CONFIRM_CROSSING: u8 = 13;
    for tick in 1..PINNED_CONFIRM_CROSSING {
        step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
        assert_eq!(
            st.gear, 2,
            "tick {tick}: nothing may commit before the confirmation crosses"
        );
    }
    step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
    assert_eq!(
        st.gear, 1,
        "the correction must commit on exactly the {PINNED_CONFIRM_CROSSING}th deficit tick"
    );
}

/// The deficit commit's landing-sign gate at its BOUNDARY. Same
/// confirmed-deficit state (F6, saturated evidence, capable F3 target) either side of the
/// reaction magnitude whose predicted 20-tick window bleed crosses m through zero
/// (boundary ≈ 82.6 kN/side at m = 0.30 with the lab coupling): below it the correction
/// commits; above it the predicted landing is backward, the commit refuses, and the
/// pending correction HOLDS the gear.
#[test]
fn deficit_commit_honors_landing_sign_boundary() {
    let tp = tiger_tp();
    assert_eq!(
        tp.shift_ticks, 20,
        "the authored window must stay realistic"
    );
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    let run = |r_side: f32| -> TransmissionState {
        let mut st = TransmissionState {
            gear: 6,
            demand_n: 150_000.0,
            demand_initialized: true,
            grade_confirm_ticks: 20,
            ..fresh(&tp)
        };
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [0.30; 2], [r_side; 2]),
        );
        st
    };
    let commits = run(70_000.0);
    assert!(
        commits.gear < 6,
        "a landing predicted FORWARD must commit the confirmed correction (got F{})",
        commits.gear
    );
    let refuses = run(90_000.0);
    assert_eq!(
        refuses.gear, 6,
        "a landing predicted BACKWARD must refuse and hold — the near-rest hill-hold \
             seam owns the arrest"
    );
    assert_eq!(refuses.shift_ticks, 0, "no window may start on the refusal");
}

/// Priority rule: while shaft AND crank read past the ceiling the crank
/// rescue owns the decision tick. The over-rev gate already makes every capability
/// downshift ILLEGAL at such a shaft (a lower gear only raises rpm), so the fixture pins
/// the observable half: a SATURATED confirmed deficit (huge demand EMA, evidence far past
/// the 13-tick bar) must neither swallow the tick nor divert it — the protective upshift
/// commits through it.
#[test]
fn crank_rescue_not_preempted_by_deficit_machinery() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 2;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let overrun_rpm = tp.max_curve_rpm() + 100.0;
    let shaft = overrun_rpm * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[4];
    let mut st = TransmissionState {
        gear: 5,
        omega_e: overrun_rpm * RPM_TO_RAD,
        demand_n: 300_000.0,
        demand_initialized: true,
        grade_confirm_ticks: 60,
        ..fresh(&tp)
    };
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [shaft; 2], [150_000.0; 2]),
    );
    assert_eq!(
        st.gear, 6,
        "the crank rescue must commit through a saturated deficit, not be pre-empted"
    );
    assert_eq!(
        st.shift_ticks,
        tp.shift_ticks - 1,
        "the rescue must have paid its window"
    );
}

/// The near-rest anti-rollback seam exists on the REVERSE
/// ladder too. Backing up a DERIVED 20-degree grade in over-tall R4 (negative launch
/// reserve at rest — the reverse mirror of `hill_hold_engages_selects_launch_gear...`):
/// held S must latch the hold, Direct-address the capable launch gear, keep both belts
/// stopped through the paid cut, then release into a BACKWARD launch. Before the fix the
/// latch was gated `!st.reverse`, so the documented "decelerate into the hill-hold seam"
/// fallback for a sign-crossed reverse deficit simply did not exist — the tank
/// lugged/brake-cycled in the tall gear instead of handing off.
#[test]
fn reverse_ladder_hill_hold_engages_and_launches() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 2;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
    let mut st = TransmissionState {
        gear: 4,
        reverse: true,
        ..fresh(&tp)
    };

    // Reverse-climb reactions are NEGATIVE (demand projects through dir = −1).
    let first = step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(-1.0, 0.0, [0.0; 2], [-demand / 2.0; 2]),
    );
    assert!(
        st.hill_hold,
        "negative reverse launch reserve must engage hill hold"
    );
    assert!(
        st.gear < 4,
        "the hold must address a capable reverse launch gear (got R{})",
        st.gear
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
            &input(-1.0, 0.0, speeds, [-demand / 2.0; 2]),
        );
        speeds = report.next_speeds;
        if !st.hill_hold {
            released = Some(tick);
            break;
        }
    }
    assert!(
        released.is_some(),
        "the capable reverse launch gear must release the hold after its window"
    );
    assert!(
        speeds[0] < 0.0 && speeds[1] < 0.0,
        "release must begin a BACKWARD launch up the grade (belts {speeds:?})"
    );

    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(0.0, 0.0, speeds, [-demand / 2.0; 2]),
    );
    assert!(
        !st.hill_hold,
        "command release always disengages the reverse hold too"
    );
}

/// The FULL closed-loop pipeline the branch test
/// (`steep_grade_after_upshift_corrects_before_lug`) seeds by hand: a REAL band upshift
/// (paying the full-predicate confirmation live, tick PINNED), the authored 20-tick
/// window with the demand EMA provably FROZEN, post-window EMA recovery pinned to the
/// steep asymptote, the 13-tick deficit confirmation, and the correction committing on
/// its exact measured tick — paced, in this fixture, by lower-gear over-rev/margin
/// legality — with the vehicle still moving forward and the crank bounded throughout.
/// A regression in window freezing, EMA rate, confirmation duration, over-rev legality,
/// or margin arithmetic moves a pinned number instead of hiding in an
/// eventually-happens bound.
///
/// Fixture note: `fp.inertia` is raised to 45 t per side as the closed loop's stand-in
/// for the COUPLED per-side mass (belt + hull share) — the pure belt harness otherwise
/// bleeds a 20-tick cut several times faster than the real sim, which no realistic
/// scenario survives.
#[test]
fn steep_grade_full_loop_upshift_freeze_recover_confirm_correct() {
    let tp = tiger_tp();
    assert_eq!(
        tp.shift_ticks, 20,
        "the authored window must stay realistic"
    );
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 45_000.0;
    fp.grip_stiffness = forces::grip_stiffness(0.9, 57_000.0 * 9.81);
    let light = 6_000.0f32; // per-side approach load
    let steep = 57_000.0 * 9.81 * 25.0_f32.to_radians().sin() / 2.0; // per side
    let start_rpm = tp.shift_up_rpm - 50.0;
    let mut st = TransmissionState {
        gear: 5,
        omega_e: start_rpm * RPM_TO_RAD,
        ..fresh(&tp)
    };
    let mut speeds = [start_rpm * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[4]; 2];

    let mut upshift_tick = None;
    let mut frozen_demand = None;
    let mut correction_tick = None;
    for tick in 0..400 {
        let r = if upshift_tick.is_some() { steep } else { light };
        let rep = step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, speeds, [r; 2]),
        );
        speeds = rep.next_speeds;
        if upshift_tick.is_none() && st.gear == 6 {
            // The REAL upshift paid its confirmation live: at least the confirm window
            // must have elapsed first.
            assert!(
                tick + 1 >= UPSHIFT_CONFIRM_TICKS as usize,
                "the upshift committed before its sustained-speed confirmation ({tick})"
            );
            upshift_tick = Some(tick);
            frozen_demand = Some(st.demand_n.to_bits());
        } else if let Some(up) = upshift_tick
            && tick < up + tp.shift_ticks as usize
        {
            // Scoped to the UPSHIFT's own paid window (the later correction opens its
            // own): the observer only samples on ticks it ENTERS with no window in
            // flight, so the EMA holds the commit tick's bits until the tick after the
            // window drains.
            assert_eq!(
                Some(st.demand_n.to_bits()),
                frozen_demand,
                "tick {tick}: the shift window must FREEZE the demand EMA"
            );
        } else if let Some(up) = upshift_tick
            && (tick == up + tp.shift_ticks as usize || tick == up + tp.shift_ticks as usize + 1)
        {
            // EMA-rate pins: the FIRST two unfrozen samples after the
            // window follow the 1/8 EMA exactly from the frozen 12 kN toward the 2·steep
            // asymptote — MEASURED 40 039 then 64 574 (±10 covers f32 accumulation).
            let expect = if tick == up + tp.shift_ticks as usize {
                40_039.0
            } else {
                64_574.0
            };
            assert!(
                (st.demand_n - expect).abs() < 10.0,
                "tick {tick}: post-window EMA recovery moved (got {:.1}, pinned {expect})",
                st.demand_n
            );
        }
        if let Some(up) = upshift_tick
            && correction_tick.is_none()
            && st.gear < 6
        {
            correction_tick = Some(tick);
            assert!(
                matches!(st.scheduler, SchedulerState::GradeShift { from: 6, to: 3 }),
                "the correction must be the grade shift F6 -> F3 (got {:?})",
                st.scheduler
            );
            // EMA recovery pin: by the commit the observer has fully re-acquired the
            // steep grade — within 1% of the 2 x steep per-side asymptote.
            assert!(
                st.demand_n >= 0.99 * (2.0 * steep),
                "the demand EMA must have recovered the steep grade before the \
                     correction (got {:.0} of {:.0})",
                st.demand_n,
                2.0 * steep
            );
            let m = (speeds[0] + speeds[1]) / 2.0;
            assert!(
                m > 0.0,
                "the correction must land while still moving forward (m = {m:.3})"
            );
            assert!(
                tick > up + tp.shift_ticks as usize,
                "the correction cannot precede the paid window's end"
            );
            break;
        }
    }
    // MEASURED pins for this deterministic fixture — exact ticks, so a
    // regression in any pipeline stage moves a number instead of hiding in an
    // eventually-happens bound:
    //   * upshift on tick 11: ~3 ticks of drive to cross the band from 50 rpm
    //     below it, then the 8-tick full-predicate confirmation — the light
    //     approach load keeps landing and reserve passing throughout, so the counted
    //     predicate is continuously true from the band crossing;
    //   * correction on tick 75: the PACING gate is lower-gear
    //     LEGALITY (F4 sits margin-short at speed and everything below is over-rev/
    //     fuel-cut-illegal until the deficit bleeds m to ~1.4 m/s, where F3 clears both
    //     the over-rev gate and the reserve margin and the correction commits on that
    //     first legal tick). It moved 9 ticks: 5 from the earlier upshift (window end
    //     now tick 31; EMA recovery, 13-tick deficit confirmation, and dwell expiry all
    //     anchor to it), plus 4 because the upshift now commits off 5 fewer drive ticks
    //     — a slightly lower entry speed onto the steep grade, so m bleeds to the
    //     F3-legal speed sooner. The M2 asymmetric demand fall does not touch this
    //     trace — the post-window EMA only RISES here (12 kN → 2·steep), and the rise
    //     path is bit-identical. (The deep override's dwell-crossing behavior is pinned
    //     separately in `deep_deficit_overrides_post_upshift_deferral`.)
    assert_eq!(
        upshift_tick,
        Some(11),
        "the confirmed upshift's tick moved — confirmation duration or drive arithmetic \
             changed"
    );
    assert_eq!(
        correction_tick,
        Some(75),
        "the correction's tick moved — EMA rate, confirmation, over-rev legality, or \
             margin arithmetic changed"
    );
}

/// Field evidence (~9000 rpm crank following the belt on the rescaled
/// steep world): the over-rev slip guard — the stall guard's mirror — bounds the crank at
/// `max_curve_rpm + OVERREV_MARGIN_RPM` however fast the belt is back-driven, and the
/// slipping clutch still transmits motoring drag (positive mean-axis force against the
/// backward belt: retardation degrades gracefully, it does not vanish). Top REVERSE gear,
/// so no rescue gear exists — the guard is the ONLY bound.
#[test]
fn overrun_slip_guard_bounds_crank_at_top_gear() {
    let tp = tiger_tp();
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let guard = (tp.max_curve_rpm() + OVERREV_MARGIN_RPM) * RPM_TO_RAD;
    // The field-class overrun: a belt speed implying ~6000 rpm in top reverse gear.
    let m = -(6_000.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_rev[3]);
    let mut st = TransmissionState {
        gear: 4,
        reverse: true,
        omega_e: tp.engine.governed_rpm * RPM_TO_RAD,
        demand_initialized: true,
        ..fresh(&tp)
    };
    let mut rep = TransmissionReport::default();
    for tick in 0..64 {
        rep = step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, [m; 2], [40_000.0; 2]),
        );
        assert!(
            st.omega_e <= guard + 1e-3,
            "tick {tick}: crank {:.0} rpm blew past the over-rev guard point ({:.0})",
            st.omega_e / RPM_TO_RAD,
            guard / RPM_TO_RAD
        );
        assert_eq!(
            st.gear, 4,
            "top reverse gear: the guard, not a shift, owns this"
        );
    }
    // Magnitude, not just sign: at the guard point the steady slipping
    // clutch transmits EXACTLY the guard-point motoring drag — in a STRAIGHT overrun
    // both sprocket powers are regenerative (`net ≤ 0`), so the power gate cannot scale
    // it. Literal: τ_drag(3100) = 462.5·(1/3 + 2/3·(3100/1550)) = 770.8 N·m through
    // k(R4) = ω_rated/v4 = 314.16/2.556 = 122.9 per m → 94.76 kN mean-axis, 47.38 kN per
    // side. The ±150 N slack covers f32 accumulation only.
    for (i, force) in rep.forces.iter().enumerate() {
        assert!(
            (*force - 47_382.0).abs() < 150.0,
            "side {i}: the guard-point steady force must be the FULL motoring drag \
                 through the gearing (got {force:.0} N, pinned ≈ 47382)"
        );
    }
}

/// The ordinary band upshift arms only after [`UPSHIFT_CONFIRM_TICKS`] CONSECUTIVE ticks of
/// the FULL ordinary-upshift predicate (band, landing, and reserve all pass here — the
/// unloaded operating point keeps them true, so the band excursion is the flipping
/// sub-condition), and the evidence HARD-resets on any non-qualifying tick. Two
/// recoil-class excursions of confirm − 1 ticks each (a fired shell's ~0.14 m/s shove
/// decays through the band well inside the window on any grade the tank cannot sustain)
/// must commit NOTHING — the second proves the reset, since a leaky counter would have
/// accumulated across both — while genuinely sustained speed commits on exactly the
/// confirm-th consecutive tick.
#[test]
fn upshift_confirmation_rejects_transient_band_excursions() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let g1 = tp.gears_fwd[0];
    let v_hi = (tp.shift_up_rpm + 80.0) * RPM_TO_RAD * tp.sprocket_radius / g1;
    let v_lo = (tp.shift_up_rpm - 100.0) * RPM_TO_RAD * tp.sprocket_radius / g1;
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

    for burst in 0..2 {
        for tick in 0..(UPSHIFT_CONFIRM_TICKS - 1) {
            at(&mut st, v_hi);
            assert_eq!(
                st.gear, 1,
                "burst {burst}, tick {tick}: a sub-confirmation excursion must not shift"
            );
        }
        at(&mut st, v_lo);
        assert_eq!(st.gear, 1, "burst {burst}: back below the band — still F1");
        assert_eq!(
            st.band_confirm_ticks, 0,
            "burst {burst}: one below-band tick must HARD-reset the evidence"
        );
    }

    let mut committed_at = None;
    for tick in 0..(2 * UPSHIFT_CONFIRM_TICKS) {
        at(&mut st, v_hi);
        if st.gear == 2 {
            committed_at = Some(tick + 1);
            break;
        }
    }
    assert_eq!(
        committed_at,
        Some(UPSHIFT_CONFIRM_TICKS),
        "sustained speed must commit on exactly the confirm-th consecutive tick"
    );
}

/// The upshift confirmation must read THIS tick's hysteretic
/// detent state, not the previous tick's. Seven qualifying straight ticks bring the
/// counter to N − 1; the player then crosses WIDE_ON on the very tick that would commit.
/// The stale read (detent updated below the scheduler) incremented to N and committed an
/// ordinary upshift on the exact tick the turn began — then the same invocation applied
/// the λ constraint in the NEW gear, where the landing predictor is explicitly invalid.
/// One detent truth per tick: the engage tick must HARD-reset the evidence and refuse
/// the shift. The straight control proves the fixture sat one tick from committing.
#[test]
fn detent_engage_on_the_committing_tick_resets_confirmation() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let g1 = tp.gears_fwd[0];
    let v_hi = (tp.shift_up_rpm + 80.0) * RPM_TO_RAD * tp.sprocket_radius / g1;
    let at = |st: &mut TransmissionState, steer: f32| {
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            st,
            &input(1.0, steer, [v_hi, v_hi], [0.0, 0.0]),
        );
    };

    // Control: the identical straight stream commits on exactly the confirm-th tick.
    let mut control = fresh(&tp);
    for _ in 0..UPSHIFT_CONFIRM_TICKS {
        at(&mut control, 0.0);
    }
    assert_eq!(
        control.gear, 2,
        "the straight control must commit on the confirm-th tick — otherwise the \
             detent tick below proves nothing"
    );

    let mut st = fresh(&tp);
    for tick in 0..(UPSHIFT_CONFIRM_TICKS - 1) {
        at(&mut st, 0.0);
        assert_eq!(
            st.gear, 1,
            "tick {tick}: still confirming, nothing may commit"
        );
    }
    assert_eq!(
        st.band_confirm_ticks,
        UPSHIFT_CONFIRM_TICKS - 1,
        "the fixture must sit exactly one tick from committing"
    );
    // The committing tick: the stick crosses WIDE_ON — the detent engages THIS tick.
    at(&mut st, 0.3);
    assert_eq!(
        st.steer_step, 1,
        "the wide detent must have engaged on this very tick"
    );
    assert_eq!(
        st.gear, 1,
        "no ordinary upshift may commit on the tick the detent engages"
    );
    assert_eq!(st.shift_ticks, 0, "no shift window may start");
    assert_eq!(
        st.band_confirm_ticks, 0,
        "the engage tick must HARD-reset the confirmation evidence"
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
    let mut st = confirmed(&tp);
    // rpm(gear1) at m: m·G1/r_s in rad/s → rpm. G1 ≈ ω_rated·r_s/v1.
    let g1 = tp.gears_fwd[0];
    let m_for = |rpm: f32| rpm * RPM_TO_RAD * tp.sprocket_radius / g1;

    // Above the up band → one upshift, then the window holds further decisions.
    // Up band + 80 (1780 rpm): comfortably past the band AND past the fix-1a landing gate
    // (unloaded landing 1780 × 8/12.7 ≈ 1121 rpm ≥ down band 900 + margin 150).
    let v = m_for(tp.shift_up_rpm + 80.0);
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
    let v_mid = (tp.shift_up_rpm + tp.shift_down_rpm) / 2.0 * RPM_TO_RAD * tp.sprocket_radius / g2;
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
    let v_low = (tp.shift_down_rpm - 50.0) * RPM_TO_RAD * tp.sprocket_radius / g2;
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
/// torque-cut window still lands above the down band + POSTSHIFT_MARGIN_RPM — under the
/// HONEST window model (decayed reaction over the coupled mass).
/// Same operating point (1780 rpm in gear 1, landing ratio ≈ 1121 rpm unloaded), three
/// loads:
///   * unloaded (flat, D ≈ 0): the predictor is a near-no-op and the shift engages —
///     anti-hunting on the flat is carried by the validator-guaranteed band geometry
///     (1121 ≥ 900 + 150), not by refusals;
///   * a working grade load (25 kN/side): bleed ≈ 25 000 × 4.94 ticks / 64 / 29 250 kg
///     ≈ 0.09 m/s → landing ≈ 1075 rpm ≥ 1050 — the shift PROCEEDS. This exact load was
///     the old test's "must refuse" premise, and that premise WAS the bug: the frozen
///     predictor charged 20 full ticks against the bare belt inertia (≈ 0.98 m/s) and
///     refused every loaded climb shift (Tiger telemetry: F1→F2 illegal above ~0.6° of
///     slope, governor-pinned forever on 10%);
///   * a genuinely arresting load (60 kN/side, ≈ 27° for the lab vehicle): bleed
///     ≈ 0.22 m/s → landing ≈ 1010 rpm < 1050 — refused. The landed-in-band cases
///     cannot hunt: the landing clears the down band by ≥ the margin and the reversal
///     dwell blocks the opposite shift for 32 post-window ticks besides.
#[test]
fn upshift_landing_gate_blocks_shift_cut_hunting() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let g1 = tp.gears_fwd[0];
    let v = (tp.shift_up_rpm + 80.0) * RPM_TO_RAD * tp.sprocket_radius / g1;

    let mut st = confirmed(&tp);
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v, v], [0.0, 0.0]),
    );
    assert_eq!(st.gear, 2, "unloaded, the operating point upshifts");

    // Working grade: the honest predictor must NOT refuse (the old frozen model did).
    // Seed a small settled demand so the refusal-vs-pass distinction is the LANDING
    // gate's, not the reserve gate's.
    let mut st = TransmissionState {
        demand_n: 0.0,
        demand_initialized: true,
        ..confirmed(&tp)
    };
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v, v], [25_000.0, 25_000.0]),
    );
    assert_eq!(
        st.gear, 2,
        "a working grade load must not refuse the upshift — that refusal was the bug"
    );

    // Arresting load: the predicted landing falls inside the down band — refused.
    let mut st = TransmissionState {
        demand_n: 0.0,
        demand_initialized: true,
        ..confirmed(&tp)
    };
    step(
        TransmissionMode::Hybrid,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v, v], [60_000.0, 60_000.0]),
    );
    assert_eq!(
        st.gear, 1,
        "a landing predicted inside the down band must still refuse the upshift"
    );
    assert_eq!(st.shift_ticks, 0, "no window may have started");
}

/// Terrain regression: at the REALISTIC (authored) 20-tick window a
/// mild-grade climb must upshift F1→F2 at its trigger rpm. Tiger tables with Tiger-weight
/// grip coupling; the reaction is the grade demand itself (`W·sin θ / 2` per side). Honest
/// window bleed at 2.5° ≈ 12 200 N × 4.94 effective ticks / 64 Hz / 44 500 kg ≈ 0.02 m/s →
/// landing ≈ 1477 rpm ≥ down band + margin (1150 + 150). The old frozen predictor charged
/// all 20 ticks of full reaction against the bare belt inertia and refused — measured:
/// F1→F2 was mathematically illegal above ~0.6° of slope, and a 10% climb sat
/// governor-pinned in F1 forever. 5° (≈ 24.4 kN/side, the load class the old tests called
/// "must refuse") must upshift too: a real Tiger upshifts there.
#[test]
fn mild_grade_upshift_proceeds_at_realistic_window() {
    let tp = tiger_tp();
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    fp.grip_stiffness = forces::grip_stiffness(0.9, 57_000.0 * 9.81);
    assert_eq!(
        tp.shift_ticks, 20,
        "the authored window must stay realistic"
    );
    for grade_deg in [2.5f32, 5.0] {
        let demand = 57_000.0 * 9.81 * grade_deg.to_radians().sin();
        let trigger_m =
            (tp.shift_up_rpm + 50.0) * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[0];
        let mut st = TransmissionState {
            demand_n: demand,
            demand_initialized: true,
            ..confirmed(&tp)
        };
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [trigger_m; 2], [demand / 2.0; 2]),
        );
        assert_eq!(
            st.gear, 2,
            "a {grade_deg} degree grade must not refuse F1 -> F2 at the trigger rpm"
        );
        // The step that commits also spends the window's first tick.
        assert_eq!(
            st.shift_ticks,
            tp.shift_ticks - 1,
            "the paid window must start"
        );
    }
}

/// Latch-out regression at the ceiling: the protective upshift must fire past `max_curve_rpm` even while SERVICE
/// BRAKING (opposing throttle), with a STEER detent engaged, and at a shaft/crank speed
/// deep past the fuel cut where `available_force_in_gear` reads ZERO in every gear. Each
/// of the old gates — `service == 0.0`, `!detent_turn`, and `next_reserve >= margin` —
/// individually disabled the rescue exactly where descending needs it (braking/steering
/// downhill), and the reserve gate did so PERMANENTLY above the cut, since modeled force
/// is zero there while rpm only rises.
#[test]
fn protective_upshift_fires_under_brake_steer_and_fuel_cut() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 2;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    // Past the mechanical-protection ceiling (and so also past the governor-cut end).
    let overrun_rpm = tp.max_curve_rpm() + 200.0;
    let g4 = tp.gears_fwd[3];
    let shaft = overrun_rpm * RPM_TO_RAD * tp.sprocket_radius / g4;
    assert_eq!(
        available_force_in_gear(&tp, &fp, shaft, g4),
        0.0,
        "fixture must sit past the fuel cut, where every gear's modeled force is zero"
    );
    let mut st = TransmissionState {
        gear: 4,
        steer_step: 2,
        omega_e: overrun_rpm * RPM_TO_RAD,
        demand_n: 50_000.0,
        demand_initialized: true,
        ..fresh(&tp)
    };
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(-1.0, 0.6, [shaft; 2], [-40_000.0; 2]),
    );
    assert_eq!(
        st.gear, 5,
        "the protective upshift must fire despite service brake + steer + fuel-cut zero \
             reserve"
    );
    // The step that commits also spends the window's first tick.
    assert_eq!(
        st.shift_ticks,
        tp.shift_ticks - 1,
        "a paid interruption window must start"
    );
}

/// Above the corroboration floor (governed +
/// [`CRANK_CORROBORATION_MARGIN_RPM`]) the crank must corroborate the shaft before the
/// ORDINARY arm may price it. A belt spike to F6 governed + 200 rpm with the crank at
/// idle is a clutch-infeasible transient (locked driving keeps crank == shaft rpm;
/// propulsive clutch slip puts the crank ABOVE the shaft, never below): under full
/// throttle the ORDINARY arm would otherwise price a fictitious operating point and
/// commit F6→F7 (landing ≈ 1850 ≥ floor, reserve clears against the light demand);
/// coasting, nothing may fire (the box holds gear on overrun below the
/// ceiling, and a transient must not fire the last resort either — it reads the same
/// both-speeds corroboration at the ceiling). Both must refuse. The same full-throttle
/// state with a corroborating crank commits — the guard tests the crank, not the
/// operating point.
#[test]
fn upshift_arms_refuse_overrun_shaft_without_crank() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 2;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let spike_rpm = tp.engine.governed_rpm + 200.0;
    let shaft = spike_rpm * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[5];
    for throttle in [1.0, 0.0] {
        let mut st = TransmissionState {
            gear: 6,
            demand_initialized: true,
            // Confirmation pre-paid: the refusal under test must be the CRANK
            // corroboration's, not the sustained-speed dwell's.
            ..confirmed(&tp) // crank at authored idle — nowhere near the spike
        };
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(throttle, 0.0, [shaft; 2], [0.0; 2]),
        );
        assert_eq!(
            st.gear, 6,
            "throttle {throttle}: an uncorroborated overrun shaft must not upshift"
        );
        assert_eq!(
            st.shift_ticks, 0,
            "throttle {throttle}: no window may start"
        );
    }
    let mut st = TransmissionState {
        gear: 6,
        omega_e: spike_rpm * RPM_TO_RAD,
        demand_initialized: true,
        ..confirmed(&tp)
    };
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [shaft; 2], [0.0; 2]),
    );
    assert_eq!(st.gear, 7, "a corroborated overrun must still upshift");
}

/// The case the deficit restructure exists for: a CONFIRMED deficit whose selector finds NO legal lower gear must fall through
/// and let a legal ordinary upshift commit. Operating point: F5 pushed into the fuel-cut
/// dead zone just BELOW the overrun floor (governed + 120: torque_at = 0 → current
/// reserve < 0 → the deficit confirms, but governed + 120 < governed + 150 so no
/// protective arm), light demand, every lower gear over-revved (selector None), while F6
/// drops the rpm back into the meaty curve and clears reserve + landing. The old
/// unconditional `if confirmed_deficit` pre-emption silently owned this tick forever.
#[test]
fn confirmed_deficit_without_target_falls_through_to_legal_upshift() {
    let mut tp = tiger_tp();
    tp.shift_ticks = 2;
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let dead_zone_rpm = tp.engine.governed_rpm + 120.0;
    let shaft = dead_zone_rpm * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[4];
    let demand = 20_000.0;
    assert_eq!(
        available_force_in_gear(&tp, &fp, shaft, tp.gears_fwd[4]),
        0.0,
        "fixture must sit in the fuel-cut dead zone (zero modeled force in-gear)"
    );
    let mut st = TransmissionState {
        gear: 5,
        omega_e: dead_zone_rpm * RPM_TO_RAD,
        demand_n: demand,
        demand_initialized: true,
        grade_confirm_ticks: GRADE_CONFIRM_TICKS - 1,
        ..confirmed(&tp)
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
        "a confirmed deficit with no downshift target must not swallow the legal upshift"
    );
}

/// The anti-hunting ARCHITECTURE across a whole
/// realistic shift, not just its committing tick. Closed-loop belt (speeds fed back under
/// a constant 2.5°-class reaction), Tiger tables, the authored 20-tick window. In this
/// belt-only harness the cut bleeds the FULL frozen reaction (≈ 0.24 m/s — deliberately
/// worse than the predictor's coupled-mass estimate, since there is no hull mass here),
/// so the F2 landing dips BELOW the down band: the reversal dwell must carry the recovery
/// — the gear may only ever climb (no 2→1 reversal on ANY tick) — and drive must lift the
/// operating point back above the band well inside the dwell.
#[test]
fn shift_window_and_dwell_survive_landing_below_down_band() {
    let tp = tiger_tp();
    assert_eq!(
        tp.shift_ticks, 20,
        "the authored window must stay realistic"
    );
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    let r_side = 12_200.0; // ≈ 2.5° Tiger grade per side
    let trigger_m = (tp.shift_up_rpm + 50.0) * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[0];
    let mut st = confirmed(&tp);
    let mut speeds = [trigger_m; 2];
    let mut prev_gear = 1u8;
    let mut min_rpm_in_2 = f32::INFINITY;
    let total = tp.shift_ticks as usize + REVERSAL_DWELL_TICKS as usize + 8;
    for tick in 0..total {
        let rep = step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, speeds, [r_side; 2]),
        );
        speeds = rep.next_speeds;
        if tick == 0 {
            assert_eq!(st.gear, 2, "the trigger tick must commit F1 -> F2");
        }
        assert!(
            st.gear >= prev_gear,
            "tick {tick}: gear reversal {prev_gear} -> {} — the dwell failed",
            st.gear
        );
        prev_gear = st.gear;
        if st.gear == 2 && st.shift_ticks == 0 {
            let rpm2 =
                (speeds[0] + speeds[1]) / 2.0 * tp.gears_fwd[1] / tp.sprocket_radius / RPM_TO_RAD;
            min_rpm_in_2 = min_rpm_in_2.min(rpm2);
        }
    }
    assert!(
        min_rpm_in_2 < tp.shift_down_rpm,
        "the landing must actually dip below the down band or this test bites nothing \
             (min F2 rpm {min_rpm_in_2:.0})"
    );
    let final_ratio = tp.gears_fwd[(st.gear - 1) as usize];
    let final_rpm = (speeds[0] + speeds[1]) / 2.0 * final_ratio / tp.sprocket_radius / RPM_TO_RAD;
    assert!(
        final_rpm >= tp.shift_down_rpm,
        "the recovery must end above the down band (F{} at {final_rpm:.0} rpm)",
        st.gear
    );
}

/// Contact-driven demand acquisition at unit level (the 20° approach
/// fixture now SEEDS its EMA by declaration, so its scope is scheduler behavior GIVEN the
/// demand; THIS test pins that real reactions do initialize and track the observer): the
/// first grounded sample seeds the EMA exactly (bit-equal), and a sustained changed load
/// converges it — the 1/8 EMA leaves (7/8)³² ≈ 1.4% of the step after 32 ticks.
#[test]
fn demand_ema_seeds_from_first_sample_and_converges() {
    let tp = tiger_tp();
    let mut fp = lab_fp();
    fp.engine_force = 250_000.0;
    fp.inertia = 16_000.0;
    // Mid-band F4 operating point: no band, reserve, or hill-hold decision interferes.
    let shaft = 1_700.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[3];
    let mut st = TransmissionState {
        gear: 4,
        omega_e: 1_700.0 * RPM_TO_RAD,
        ..fresh(&tp)
    };
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [shaft; 2], [40_000.0; 2]),
    );
    assert_eq!(
        st.demand_n.to_bits(),
        80_000.0f32.to_bits(),
        "the first grounded sample must seed the EMA exactly"
    );
    for _ in 0..32 {
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [shaft; 2], [60_000.0; 2]),
        );
    }
    assert!(
        (st.demand_n - 120_000.0).abs() < 0.02 * 120_000.0,
        "sustained reactions must converge the EMA (got {:.0})",
        st.demand_n
    );
}

/// The load-time band validator must include the runtime landing
/// margin. The Tiger's ORIGINAL shipped triple (2300 / 1400) cleared the bare down band,
/// but its widest-step landing (2300 × 2.8/4.3 ≈ 1498 rpm) sat 52 rpm SHORT of the runtime
/// gate's `1400 + POSTSHIFT_MARGIN_RPM` floor — legal-looking authoring that could never
/// shift at its own trigger rpm (it only ever shifted by over-running to the governor).
/// Incompatible triples must fail loudly at asset load; the corrected 1150 band passes.
#[test]
fn validator_rejects_band_triple_inside_postshift_margin() {
    let mut a = tiger_authoring();
    a.shift_down_rpm = 1400.0;
    let err = TransmissionParams::from_authoring(&a)
        .expect_err("a triple violating the landing margin must fail at load");
    assert!(
        err.to_string().contains("hysteresis"),
        "the rejection must name the band-hysteresis field: {err}"
    );
    TransmissionParams::from_authoring(&tiger_authoring())
        .expect("the corrected shipped triple must validate");
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

/// Stage A: the REVERSE-ladder mirror of the backslide test. Driving in
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
        // 900 like lab_tp: the validator now includes POSTSHIFT_MARGIN_RPM.
        shift_down_rpm: 900.0,
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

/// Stage A: coasting to rest in a cruise gear must
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
    let mut st = confirmed(&tp);
    let at = |st: &mut TransmissionState, v: f32| {
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            st,
            &input(1.0, 0.0, [v, v], [0.0, 0.0]),
        );
    };
    // 1 → 2 commits (dwell armed) at up band + 80 (1780 rpm).
    at(&mut st, rpm_v(tp.shift_up_rpm + 80.0, tp.gears_fwd[0]));
    assert_eq!(st.gear, 2);
    // Drain the interruption window at a mid-band gear-2 speed; the dwell (32 ticks)
    // must still be live when the window (≈ 20 ticks) ends, or this test bites nothing.
    let mid_band = (tp.shift_up_rpm + tp.shift_down_rpm) / 2.0;
    for _ in 0..tp.shift_ticks {
        at(&mut st, rpm_v(mid_band, tp.gears_fwd[1]));
    }
    assert_eq!(st.gear, 2);
    assert!(st.dwell_ticks > 0, "the dwell must outlive the window");
    // SAME direction: 2 → 3 engages immediately despite the live dwell (up band + 80 =
    // 1780 rpm, landing 1780 × 12.7/20.4 ≈ 1108 ≥ 1050 clears the fix-1a gate). The
    // sustained-speed confirmation is re-seeded — the mid-band drain hard-reset it, and
    // THIS test pins dwell semantics, not the confirmation dwell.
    st.band_confirm_ticks = UPSHIFT_CONFIRM_TICKS;
    at(&mut st, rpm_v(tp.shift_up_rpm + 80.0, tp.gears_fwd[1]));
    assert_eq!(
        st.gear, 3,
        "same-direction shifts must not be dwell-blocked"
    );
    // OPPOSITE direction: drop below gear-3's down band. The downshift must wait out
    // the FULL dwell after the window — the dwell counts only outside the frozen
    // window, so the reversal engages exactly at
    // window + REVERSAL_DWELL_TICKS.
    let v_low = rpm_v(tp.shift_down_rpm - 50.0, tp.gears_fwd[2]);
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

/// Intent gate: ORDINARY band upshifts are considered only under
/// PROPULSIVE drive — the landing predictor has no brake term, so consulting it under
/// braking produced a false shift (predicted 1652 rpm on drag alone vs 1262 real under
/// the brakes) followed by a reversal cycle. The operating point sits BELOW the
/// mechanical-protection ceiling: the last-resort protective shift past `max_curve_rpm`
/// is the sole exception to this intent gate
/// (see `protective_upshift_fires_under_brake_steer_and_fuel_cut`).
#[test]
fn no_upshift_while_braking_or_coasting() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let g1 = tp.gears_fwd[0];
    let v = (tp.shift_up_rpm + 80.0) * RPM_TO_RAD * tp.sprocket_radius / g1;
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

/// Predictor-domain guard: while the L600 steering detent is engaged
/// the landing predictor has no λ/steer state, so ORDINARY band upshifts are DEFERRED
/// until the detent releases; downshifts stay allowed mid-turn. (The last-resort
/// protective shift is detent-exempt — this operating point sits below the
/// mechanical-protection ceiling.)
#[test]
fn l600_detent_defers_upshift() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let v_up = (tp.shift_up_rpm + 80.0) * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[0];
    // Detent engaged (tight) at an above-band operating point: upshift deferred.
    let mut st = TransmissionState {
        steer_step: 2,
        ..confirmed(&tp)
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
    let mut st = confirmed(&tp);
    step(
        TransmissionMode::FixedRadii,
        &fp,
        Some(&tp),
        &mut st,
        &input(1.0, 0.0, [v_up, v_up], [0.0, 0.0]),
    );
    assert_eq!(st.gear, 2, "detent released, the upshift proceeds");
    // Downshifts stay allowed mid-turn (over-rev gate permitting).
    let v_low = (tp.shift_down_rpm - 50.0) * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[2];
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

/// Releasing the steer at a standstill pivot must actively
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
        // invalidity below (the margin-inclusive validator needs 1700 × 8/20.4 ≈ 667 >
        // shift_down + 150, so the authored band sits at 500).
        shift_down_rpm: 500.0,
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

/// Regression: a tick that carries `m` through zero must not project the
/// constraint onto the pre-tick |m| branch — that enforces `d = s·κ·m` on the wrong
/// side of the cusp, flipping `d` AGAINST the commanded steer for a tick (a yaw
/// impulse, and ringing if m chatters around zero). The scenario: slow forward
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

/// Regression, half 1: the parking brake SETTLES creep instead of freezing it.
/// The old `B = clamp(R − Q, ±cap)` at a small positive belt speed with `R > Q` set
/// `v̇ = 0` exactly — positive brake work cancelling grip and drag, preserving creep
/// forever. The stop-force law lands the belt at zero.
#[test]
fn parking_brake_settles_creep() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    // Creep below the latch threshold, zero command, a ground reaction R > Q inside
    // capacity.
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

/// Regression, half 2: past a capacity breach the latched parking brake stays
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
/// physical-power split, from an asymmetric rolling start with a hard steer command at gentle
/// throttle (`F_s ≫ F_p`, `m > d > 0`): the case where one SPROCKET's power is negative
/// while both MODAL powers read positive, so the modal split never charged η.
#[test]
fn energy_bound_no_free_energy() {
    let (fp, tp) = (lab_fp(), lab_tp());
    for (mode, throttle, steer, seed) in [
        (TransmissionMode::Hybrid, 1.0, 0.0, [0.0f32, 0.0]),
        (TransmissionMode::Hybrid, 0.7, 0.6, [0.0, 0.0]),
        (TransmissionMode::Hybrid, 0.0, 1.0, [0.0, 0.0]),
        // Steer-dominant at a rolling start — inner sprocket goes negative.
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

/// The recirculation split reads the PHYSICAL sprocket powers, not the
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

/// The "cannot decelerate" regression: a forward-moving tank given
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

/// Through-zero feel: after the ladder swap commits with the hull still
/// rolling the OLD way, the held command keeps BRAKING through the declutched swap window.
/// The old seam keyed `service` on the engaged ladder alone: the swap flipped `opposing`
/// false the same tick, and the window then carried neither drive nor brake — a free-roll
/// gap that let a downhill reversal re-accelerate into its own crossing (the field-reported
/// "jumpy" stop→reverse). `back_driven_intent` closes it: drive intent on a belt still
/// moving against the engaged ladder rides the brake until the belt crosses zero.
#[test]
fn held_reversal_keeps_braking_through_swap_window() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let mut st = fresh(&tp);
    // Forward roll under the swap seam (and above the at-rest threshold), full reverse
    // held: the swap commits on the first tick and opens its interruption window.
    let m0 = DIRECTION_SWAP_SPEED - 0.05;
    let inp = input(-1.0, 0.0, [m0, m0], [0.0, 0.0]);
    let rep = step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
    assert!(st.reverse, "the swap must commit at the seam");
    assert!(st.shift_ticks > 0, "the swap must open its window");
    for (i, force) in rep.forces.iter().enumerate() {
        assert!(
            *force < 0.0,
            "side {i}: the swap tick must keep braking the forward-rolling belt \
                 (force {force:.0} N) — the free-roll gap is back"
        );
    }
    // And every remaining window tick with the belt still rolling forward brakes too.
    while st.shift_ticks > 0 {
        let rep = step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
        for (i, force) in rep.forces.iter().enumerate() {
            assert!(
                *force < 0.0,
                "side {i}: a declutched swap-window tick must brake, not free-roll \
                     (force {force:.0} N)"
            );
        }
    }
}

/// Coast intent (stage B shape): zero throttle at speed applies the DECLARED
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
        rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, [v, v], [0.0, 0.0]),
        );
    }
    assert_eq!(st.gear, 3, "mid-band coast must not shift");
    let expect = -(engine_drag(&tp, st.omega_e, 0.0) * g3 / tp.sprocket_radius) / 2.0;
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

/// DIRECT numeric pins of the motoring curve — the coupling tests above use
/// `engine_drag` as their own oracle (they pin transmission THROUGH the clutch), so the
/// curve itself is pinned here against hand-computed literals from the authored anchors.
/// Tiger: D = drag_fraction × peak = 0.25 × 1850 = 462.5 N·m, ω_mid = (600 + 2500)/2 =
/// 1550 rpm, affine law τ(ω) = D·(1/3 + (2/3)·ω/ω_mid) for every LIVE crank speed
/// (the spin-up fade sits below the stall floor, which the hard clamp makes unreachable):
///   500 (stall floor) → 253.6, 600 (idle) → 273.5, 1550 (mid-band) → 462.5 exactly,
///   2500 (governed) → 651.5, 3000 (rated/ceiling) → 750.9; a stopped crank → 0.
#[test]
fn motoring_drag_curve_pins_documented_anchors() {
    let tp = tiger_tp();
    let d = |rpm: f32| engine_drag(&tp, rpm * RPM_TO_RAD, 0.0);
    assert_eq!(d(0.0), 0.0, "a stopped crank has no motoring torque");
    let floor = tp.engine.idle_rpm - STALL_GUARD_BAND_RPM;
    for (rpm, expect) in [
        (floor, 253.6f32),
        (tp.engine.idle_rpm, 273.5),
        (1_550.0, 462.5),
        (tp.engine.governed_rpm, 651.5),
        (3_000.0, 750.9),
    ] {
        let got = d(rpm);
        assert!(
            (got - expect).abs() < 0.5,
            "τ_drag({rpm:.0} rpm) = {got:.2} N·m, pinned {expect:.1}"
        );
    }
}

/// The rise REACHES THE BELT — converged locked-coast per-side forces at two
/// held speeds in the SAME gear, pinned as literals (not via `engine_drag`). Lab gear 3
/// (G3/r_s = 33.26 per m): D = 550, ω_mid = 1200 →
///   1100 rpm: 550 × 0.9444 × 33.26 / 2 ≈ −8 639 N per side;
///   1600 rpm: 550 × 1.2222 × 33.26 / 2 ≈ −11 181 N per side —
/// the higher-speed coast brakes ≈ 29% harder through the identical gearing, which only
/// the curve's ω-term can produce (a flat drag gives identical forces at both speeds).
#[test]
fn motoring_rise_reaches_belt_at_two_speeds() {
    let (fp, tp) = (lab_fp(), lab_tp());
    let g3 = tp.gears_fwd[2];
    let converged = |rpm: f32| -> f32 {
        let v = rpm * RPM_TO_RAD * tp.sprocket_radius / g3;
        let mut st = TransmissionState {
            gear: 3,
            ..fresh(&tp)
        };
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
        assert_eq!(st.gear, 3, "the held coast must not shift");
        rep.forces[0]
    };
    let low = converged(1_100.0);
    let high = converged(1_600.0);
    assert!(
        (low + 8_639.0).abs() < 250.0,
        "1100 rpm coast per-side force {low:.0} N, pinned ≈ −8639"
    );
    assert!(
        (high + 11_181.0).abs() < 250.0,
        "1600 rpm coast per-side force {high:.0} N, pinned ≈ −11181"
    );
    assert!(
        high < low,
        "the faster coast must brake harder — the rise must reach the belt"
    );
}

/// The pivot-authority convention (the Tiger pivot-dead fix): the steering member
/// drives the two OUTPUTS differentially, so each output may carry up to the full
/// PER-OUTPUT capacity (`F_s` bounded by `2 × capacity`, `±capacity` per belt) — not
/// `±capacity/2`, which halves the yaw moment and left the Tiger under its own
/// footprint scrub. At rest under full steer the Hybrid commands full steer FORCE
/// outright (the power-limited pivot; the power scale cannot bind at v = 0),
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
            // Force command up to capacity, power-limited thereafter.
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

/// Service braking must never TELEPORT the crank through an
/// infeasible snap. An eager `exact` flag decided at the pre-brake coupling
/// solve; the brake stop-force then dropped the belt ≈ 0.23 m/s per tick
/// (120 kN / 8 t / 64 Hz) and the drift kill snapped the crank down with it —
/// ≈ 20 rad/s per tick, an implied clutch torque
/// `τ_impl = τ_free − Δω·J/dt ≈ −550 − 20·4·64 ≈ −5.7 kN·m` through a 2.86 kN·m
/// clutch. Post-fix the snap is feasibility-checked on the FINAL belt state: the crank
/// integrates honestly with the torque the clutch actually carried, so its per-tick
/// change obeys `|Δω|·J/dt ≤ capacity + |τ_free|` (braking: |τ_free| = the motoring
/// curve at the warmed crank speed, ≈ 0.8 × drag_fraction × peak at the lab's ~810 rpm
/// lock — the bound is computed from `engine_drag` below).
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
    // the crank faster than the clutch can physically pull it. The drag bound is the
    // rising motoring curve AT the warmed crank speed — the crank only falls from here
    // and the curve rises with ω, so this is the run's maximum |τ_free|.
    let drag_max = engine_drag(&tp, st.omega_e, 0.0);
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

/// The coupling seam is a LATCH with hysteresis — a belt speed
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
