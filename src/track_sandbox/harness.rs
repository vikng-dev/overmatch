//! Scripted capture harness: run the sandbox with a scenario from the `SANDBOX_HARNESS` env var,
//! record the full sim + visual-chain state per fixed tick as JSONL, then exit. Turns "look at
//! the screen" into numbers — view A/Bs, field validation, and artifact diagnosis become
//! reproducible offline analysis instead of screenshot forensics.
//!
//! `SANDBOX_HARNESS="z=-5,warmup=192,ticks=640,throttle=0.25,steer=0.4,out=/tmp/run.jsonl"`
//! - `pose` spawn preset: `lane` (default; uses `z`) or `slope_up`/`slope_down`/`slope_left`/
//!   `slope_right` — parked on the 20° pad facing up/down/across the fall line,
//! - `z` spawn lane position, `warmup` settle ticks at zero input, `ticks` recorded ticks after
//!   warmup, `throttle`/`steer` constant drive during the recorded window (`t2` +
//!   `throttle2`/`steer2` switch both at one tick: reversal slam, pivot entry, slalom flip),
//!   `view=chain` for the route-chain view (default: kinematic wrap), `out` JSONL path.
//!   Unknown keys are ignored, so historical scenario strings (e.g. `model=4`) keep working.
//!
//! Record types (one JSON object per line):
//! - `meta` — the scenario + vehicle constants.
//! - `scan` — the terrain field sampled on fixed grids at startup (horizontal depth rows at
//!   several heights along the lane; vertical profiles at interesting z): validates the oracle
//!   itself (monotonicity, plateaus, rounding) with no sim in the loop.
//! - `k` — per fixed tick: hull pose/velocity, belt speed/phase, every contact
//!   (hull-local position, load, slip), and the conformed chain (left side, hull-local) — the
//!   physics AND what the eye sees, aligned on one clock.

use std::fs::File;
use std::io::{BufWriter, Write as _};

use super::model4::TerrainField;
use super::*;

/// The parsed scenario (present only when `SANDBOX_HARNESS` is set — every harness system gates
/// on this resource existing).
#[derive(Resource)]
pub(super) struct Harness {
    /// Spawn preset: `None` = the flat lane at `z`; `Some(yaw)` = the slope pad, hull yawed
    /// `yaw` radians on the incline (0 = nose uphill).
    pose: Option<f32>,
    z: f32,
    warmup: u64,
    ticks: u64,
    throttle: f32,
    steer: f32,
    /// Second command phase: from tick `t2` (absolute, incl. warmup) the command becomes
    /// `throttle2`/`steer2` — e.g. accelerate then slam reverse (track compression), or flip
    /// the steer sign (slalom half-cycle).
    t2: u64,
    throttle2: f32,
    steer2: f32,
    /// `view=chain` runs the route-chain view instead of the kinematic wrap (the step-22 view
    /// A/B, scripted).
    chain_view: bool,
    /// `grip=off` disables the static-friction regime (the parity switch — kinetic-only
    /// law, bit-identical to the pre-grip baseline); `grip=elem` runs the per-element
    /// isotropic shear prototype; default is the shipped aggregate regime.
    grip: GripMode,
    out: String,
}

/// The open log + tick counter, created by [`harness_setup`].
#[derive(Resource)]
pub(super) struct HarnessLog {
    tick: u64,
    writer: BufWriter<File>,
}

pub(super) fn parse_env() -> Option<Harness> {
    let spec = std::env::var("SANDBOX_HARNESS").ok()?;
    let mut h = Harness {
        pose: None,
        z: 0.0,
        warmup: 192,
        ticks: 640,
        throttle: 0.0,
        steer: 0.0,
        t2: u64::MAX,
        throttle2: 0.0,
        steer2: 0.0,
        chain_view: false,
        grip: GripMode::Aggregate,
        out: "/tmp/track_harness.jsonl".into(),
    };
    for pair in spec.split(',') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key.trim() {
            "pose" => {
                h.pose = match value.trim() {
                    "slope_up" => Some(0.0),
                    "slope_down" => Some(std::f32::consts::PI),
                    "slope_left" => Some(std::f32::consts::FRAC_PI_2),
                    "slope_right" => Some(-std::f32::consts::FRAC_PI_2),
                    _ => None, // "lane" and unknown values keep the flat-lane spawn
                };
            }
            "z" => h.z = value.trim().parse().unwrap_or(0.0),
            "warmup" => h.warmup = value.trim().parse().unwrap_or(192),
            "ticks" => h.ticks = value.trim().parse().unwrap_or(640),
            "throttle" => h.throttle = value.trim().parse().unwrap_or(0.0),
            "steer" => h.steer = value.trim().parse().unwrap_or(0.0),
            "t2" => h.t2 = value.trim().parse().unwrap_or(u64::MAX),
            "throttle2" => h.throttle2 = value.trim().parse().unwrap_or(0.0),
            "steer2" => h.steer2 = value.trim().parse().unwrap_or(0.0),
            "view" => h.chain_view = value.trim() == "chain",
            "grip" => {
                h.grip = match value.trim() {
                    "off" => GripMode::Off,
                    "elem" | "elements" => GripMode::Elements,
                    _ => GripMode::Aggregate,
                };
            }
            "out" => h.out = value.trim().to_string(),
            _ => {}
        }
    }
    Some(h)
}

fn arr(vals: impl IntoIterator<Item = f32>) -> String {
    let inner: Vec<String> = vals.into_iter().map(|v| format!("{v:.4}")).collect();
    format!("[{}]", inner.join(","))
}

/// Apply the scenario (view, spawn) and write the meta + field-scan records.
pub(super) fn harness_setup(
    mut commands: Commands,
    harness: Res<Harness>,
    mut grip_switch: ResMut<GripSwitch>,
    fixed_time: Res<Time<Fixed>>,
    field: Res<TerrainField>,
    mut view: ResMut<TrackViewMode>,
    hull: Single<(&mut Transform, &mut LinearVelocity, &mut AngularVelocity), With<Hull>>,
) {
    view.kinematic = !harness.chain_view;
    grip_switch.0 = harness.grip;
    let (mut transform, mut lin, mut ang) = hull.into_inner();
    *transform = match harness.pose {
        // Slope pad: rest height along the pad NORMAL, hull tilted with the incline and
        // yawed about it (warmup settles the exact contact pose).
        Some(yaw) => {
            // +0.35 m clearance along the pad normal: a yawed hull's belt lowpoints differ
            // from the flat-lane rest pose, and spawning intersected launches the penalty
            // spring (measured: +4.3 m/s pop). A short drop settles cleanly in warmup.
            let (top, tilt) = slope_pad_pose();
            Transform::from_translation(top + tilt * Vec3::Y * (HULL_REST_Y + 0.12))
                .with_rotation(tilt * Quat::from_rotation_y(yaw))
        }
        None => Transform::from_xyz(0.0, HULL_REST_Y, harness.z),
    };
    lin.0 = Vec3::ZERO;
    ang.0 = Vec3::ZERO;

    let file = File::create(&harness.out).expect("harness out path must be writable");
    let mut writer = BufWriter::new(file);
    writeln!(
        writer,
        // `"model":4` is pinned: the sandbox hosts only the promoted field-belt model, and the
        // field stays for schema stability with existing analyzers. `schema:2` = raw/shaped
        // commands, quaternion + full angular velocity, per-side contact arrays + aggregates.
        "{{\"t\":\"meta\",\"model\":4,\"schema\":2,\"view\":\"{}\",\"pose\":\"{}\",\"slope_deg\":{},\"z\":{:.3},\"warmup\":{},\"ticks\":{},\"throttle\":{:.3},\"steer\":{:.3},\"t2\":{},\"throttle2\":{:.3},\"steer2\":{:.3},\"slew\":{},\"fixed_dt\":{},\"grip\":{},\"grip_mode\":\"{}\",\"half_tread\":{TRACK_HALF_WIDTH},\"mu\":{MU},\"lateral_ratio\":{LATERAL_GRIP_RATIO},\"slip_saturation\":{SLIP_SATURATION},\"weight\":{:.0},\"hull_rest_y\":{HULL_REST_Y},\"thickness\":{TRACK_THICKNESS}}}",
        if harness.chain_view { "chain" } else { "wrap" },
        match harness.pose {
            None => "lane".into(),
            Some(yaw) => format!("slope_yaw{yaw:.3}"),
        },
        if harness.pose.is_some() {
            SLOPE_PAD_DEG
        } else {
            0.0
        },
        harness.z,
        harness.warmup,
        harness.ticks,
        harness.throttle,
        harness.steer,
        harness.t2,
        harness.throttle2,
        harness.steer2,
        crate::track::drive::DRIVE_SLEW_PER_SECOND,
        fixed_time.timestep().as_secs_f64(),
        harness.grip != GripMode::Off,
        match harness.grip {
            GripMode::Off => "off",
            GripMode::Aggregate => "aggregate",
            GripMode::Elements => "elements",
        },
        HULL_MASS * 9.81,
    )
    .unwrap();

    // Field scans. Horizontal rows: signed
    // depth along the lane at the track line, at several heights — the terrain cross-section the
    // belly stations actually read. Vertical profiles: depth vs y at a board center / board edge /
    // gap center per washboard set — monotonicity and plateaus as numbers.
    let (z0, z1, dz) = (6.0_f32, -30.0_f32, -0.02_f32);
    let steps = ((z1 - z0) / dz) as usize;
    for y in [0.02_f32, 0.06, 0.10, 0.15, 0.20, 0.30] {
        let row: Vec<f32> = (0..=steps)
            .map(|i| field.signed_depth(Vec3::new(TRACK_HALF_WIDTH, y, z0 + dz * i as f32)))
            .collect();
        writeln!(
            writer,
            "{{\"t\":\"scan\",\"y\":{y:.3},\"z0\":{z0},\"dz\":{dz},\"d\":{}}}",
            arr(row)
        )
        .unwrap();
    }
    for z in [
        -3.0_f32, -3.2, -3.4, -10.0, -10.3, -10.75, -19.0, -19.5, -20.25,
    ] {
        let (ylo, yhi, dy) = (-0.15_f32, 0.5_f32, 0.005_f32);
        let steps = ((yhi - ylo) / dy) as usize;
        let col: Vec<f32> = (0..=steps)
            .map(|i| field.signed_depth(Vec3::new(TRACK_HALF_WIDTH, ylo + dy * i as f32, z)))
            .collect();
        // The same column through the physics' directional query (straight-down probe).
        let coldir: Vec<f32> = (0..=steps)
            .map(|i| {
                field.depth_along(
                    Vec3::new(TRACK_HALF_WIDTH, ylo + dy * i as f32, z),
                    Vec3::NEG_Y,
                )
            })
            .collect();
        writeln!(
            writer,
            "{{\"t\":\"vscan\",\"z\":{z:.3},\"y0\":{ylo},\"dy\":{dy},\"d\":{},\"dd\":{}}}",
            arr(col),
            arr(coldir)
        )
        .unwrap();
    }

    commands.insert_resource(HarnessLog { tick: 0, writer });
}

/// Drive the scenario: zero input through warmup, then the scripted RAW command — the shared
/// fixed-tick shaper slews it exactly as it slews a player's keys, so the ramp is part of the
/// tested path (a `t2` reversal takes the same ~32 ticks a keyboard reversal does). Runs in
/// FixedUpdate BEFORE the force systems, so phase boundaries (warmup end, `t2`) land on exact
/// ticks regardless of frame pacing — one half of the harness's bit-repeatability (the other is
/// the manual-duration clock). It overrides whatever `read_drive_input` wrote last frame.
pub(super) fn harness_drive(
    harness: Res<Harness>,
    log: Option<Res<HarnessLog>>,
    mut input: ResMut<RawDriveInput>,
) {
    let Some(log) = log else {
        return;
    };
    (input.0.throttle, input.0.steer) = if log.tick < harness.warmup {
        (0.0, 0.0)
    } else if log.tick >= harness.t2 {
        (harness.throttle2, harness.steer2)
    } else {
        (harness.throttle, harness.steer)
    };
}

/// Record one `k` line per fixed tick (after the model force systems), then exit when done.
pub(super) fn harness_record(
    harness: Res<Harness>,
    log: Option<ResMut<HarnessLog>>,
    hull: Single<(&Transform, &LinearVelocity, &AngularVelocity), With<Hull>>,
    raw: Res<RawDriveInput>,
    shaped: Res<ShapedDrive>,
    dynamics: Res<SideDynamics>,
    grip: Res<BeltGrip>,
    grip_mode: Res<GripSwitch>,
    grip_elements: Res<BeltGripElements>,
    contacts: Res<BeltContacts>,
    belt: Res<BeltSpeed>,
    phase: Res<BeltPhase>,
    belts: Res<ConformedBelts>,
    wheels: Query<(&RigWheel, &Suspension)>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(mut log) = log else {
        return;
    };
    let (transform, lin, ang) = *hull;
    let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    // Body-frame yaw rate: world av.y lies on slopes (codex parts-1/2 review #3).
    let yawrate_body = ang.0.dot(transform.rotation * Vec3::Y);
    let side_cmd = shaped.0.side_commands();
    let total: f32 = contacts.all().map(|c| c.load).sum();
    // Per-side contact arrays: positional prefix [x,y,z,load,slip,ny] kept from schema 1,
    // appended [load_elastic, slip_lat, f_long, f_lat].
    let side_rows = |si: usize| -> String {
        contacts.0[si]
            .iter()
            .map(|c| {
                format!(
                    "[{:.4},{:.4},{:.4},{:.0},{:.3},{:.4},{:.0},{:.3},{:.1},{:.1}]",
                    c.local.x,
                    c.local.y,
                    c.local.z,
                    c.load,
                    c.slip,
                    c.normal.y,
                    c.load_elastic,
                    c.slip_lat,
                    c.f_long,
                    c.f_lat
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    // Per-side aggregates, named: actual load, elastic load, lateral force. Longitudinal
    // force is the existing `reaction` field; per-contact slips live in the contact arrays.
    let sums = |si: usize| -> [f32; 3] {
        let cs = &contacts.0[si];
        [
            cs.iter().map(|c| c.load).sum(),
            cs.iter().map(|c| c.load_elastic).sum(),
            cs.iter().map(|c| c.f_lat).sum(),
        ]
    };
    let ([ll, lel, lfl], [rl, rel, rfl]) = (sums(0), sums(1));
    let chain_rows: Vec<String> = belts
        .left
        .iter()
        .map(|s| format!("[{:.4},{:.4}]", s.local.x, s.local.y))
        .collect();
    // Road wheels: (side sign, pivot z, smoothed lift dy, raw target) — the wheel-jumpiness
    // channel, with the un-smoothed target so smoothing lag is directly measurable.
    let mut wheel_rows: Vec<(f32, f32, f32, f32)> = wheels
        .iter()
        .filter(|(w, _)| w.kind == WheelKind::Road)
        .map(|(w, s)| {
            (
                match w.side {
                    Side::Left => -1.0,
                    Side::Right => 1.0,
                },
                s.pivot_local.z,
                s.dy,
                s.target,
            )
        })
        .collect();
    wheel_rows.sort_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).unwrap());
    let wheel_json: Vec<String> = wheel_rows
        .iter()
        .map(|(side, z, dy, target)| format!("[{side:.0},{z:.3},{dy:.4},{target:.4}]"))
        .collect();
    let k = log.tick;
    writeln!(
        log.writer,
        "{{\"t\":\"k\",\"k\":{k},\"hull\":{},\"q\":{},\"av\":{},\"pitch\":{:.5},\"yaw\":{:.5},\"yawrate\":{:.5},\"raw\":{},\"shaped\":{},\"side_cmd\":{},\"vel\":{},\"belt\":{},\"phase\":{},\"engine\":{},\"reaction\":{},\"qgrip\":[{},{}],\"load\":{},\"load_el\":{},\"flat\":{},\"sup\":{:.0},\"wheels\":[{}],\"contacts\":[[{}],[{}]],\"chain\":[{}]}}",
        arr([
            transform.translation.x,
            transform.translation.y,
            transform.translation.z
        ]),
        arr([
            transform.rotation.x,
            transform.rotation.y,
            transform.rotation.z,
            transform.rotation.w
        ]),
        arr([ang.0.x, ang.0.y, ang.0.z]),
        pitch,
        yaw,
        yawrate_body,
        arr([raw.0.throttle, raw.0.steer]),
        arr([shaped.0.throttle, shaped.0.steer]),
        arr(side_cmd),
        arr([lin.0.x, lin.0.y, lin.0.z]),
        arr([belt.left, belt.right]),
        arr([phase.get(Side::Left) as f32, phase.get(Side::Right) as f32]),
        arr(dynamics.engine),
        arr(dynamics.reaction),
        arr([grip.0[0].x, grip.0[0].y]),
        arr([grip.0[1].x, grip.0[1].y]),
        arr([ll, rl]),
        arr([lel, rel]),
        arr([lfl, rfl]),
        total,
        wheel_json.join(","),
        side_rows(0),
        side_rows(1),
        chain_rows.join(","),
    )
    .unwrap();
    // Element-regime strain telemetry (`e` line, `grip=elem` runs only — `k` lines stay
    // byte-stable for the parity gates): per side, the count of elements holding strain and
    // Σ|j| / max|j| (m). Contact-loss erasure shows as a `jsum` sawtooth with no hull motion
    // — the parking-flutter instrument (netcode review defect 1).
    if grip_mode.0 == GripMode::Elements {
        let e_side = |si: usize| -> (usize, f32, f32) {
            let js = &grip_elements.0[si];
            let n = js.iter().filter(|j| **j != Vec3::ZERO).count();
            let sum: f32 = js.iter().map(|j| j.length()).sum();
            let max = js.iter().map(|j| j.length()).fold(0.0f32, f32::max);
            (n, sum, max)
        };
        let (ln, ls, lm) = e_side(0);
        let (rn, rs, rm) = e_side(1);
        writeln!(
            log.writer,
            "{{\"t\":\"e\",\"k\":{k},\"n\":[{ln},{rn}],\"jsum\":[{ls:.6},{rs:.6}],\"jmax\":[{lm:.6},{rm:.6}]}}"
        )
        .unwrap();
    }
    log.tick += 1;
    if log.tick >= harness.warmup + harness.ticks {
        log.writer.flush().unwrap();
        info!("harness: wrote {} ticks to {}", log.tick, harness.out);
        exit.write(AppExit::Success);
    }
}
