//! Scripted capture harness: run the sandbox with a scenario from the `SANDBOX_HARNESS` env var,
//! record the full sim + visual-chain state per fixed tick as JSONL, then exit. Turns "look at
//! the screen" into numbers — model A/Bs, field validation, and artifact diagnosis become
//! reproducible offline analysis instead of screenshot forensics.
//!
//! `SANDBOX_HARNESS="model=4,z=-5,warmup=192,ticks=640,throttle=0.25,out=/tmp/run.jsonl"`
//! - `model` 1–4 (registry index), `z` spawn lane position, `warmup` settle ticks at zero input,
//!   `ticks` recorded ticks after warmup, `throttle` constant drive during the recorded window,
//!   `view=chain` for model 4's frozen Verlet-chain view (default: kinematic wrap),
//!   `out` JSONL path.
//!
//! Record types (one JSON object per line):
//! - `meta` — the scenario + vehicle constants.
//! - `scan` — model 4's terrain field sampled on fixed grids at startup (horizontal depth rows at
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
    model: Model,
    z: f32,
    warmup: u64,
    ticks: u64,
    throttle: f32,
    /// Second throttle phase: from tick `t2` (absolute, incl. warmup) the throttle becomes
    /// `throttle2` — e.g. accelerate then slam reverse (the track-compression scenario).
    t2: u64,
    throttle2: f32,
    /// `view=chain` runs model 4 with the frozen Verlet-chain view instead of the kinematic wrap
    /// (the step-22 view A/B, scripted).
    chain_view: bool,
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
        model: Model::FieldBelt,
        z: 0.0,
        warmup: 192,
        ticks: 640,
        throttle: 0.0,
        t2: u64::MAX,
        throttle2: 0.0,
        chain_view: false,
        out: "/tmp/track_harness.jsonl".into(),
    };
    for pair in spec.split(',') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key.trim() {
            "model" => {
                let idx: usize = value.trim().parse().unwrap_or(4);
                h.model = MODELS[(idx - 1).min(MODELS.len() - 1)];
            }
            "z" => h.z = value.trim().parse().unwrap_or(0.0),
            "warmup" => h.warmup = value.trim().parse().unwrap_or(192),
            "ticks" => h.ticks = value.trim().parse().unwrap_or(640),
            "throttle" => h.throttle = value.trim().parse().unwrap_or(0.0),
            "t2" => h.t2 = value.trim().parse().unwrap_or(u64::MAX),
            "throttle2" => h.throttle2 = value.trim().parse().unwrap_or(0.0),
            "view" => h.chain_view = value.trim() == "chain",
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

/// Apply the scenario (model, spawn) and write the meta + field-scan records.
pub(super) fn harness_setup(
    mut commands: Commands,
    harness: Res<Harness>,
    field: Res<TerrainField>,
    mut active: ResMut<ActiveModel>,
    mut view: ResMut<TrackViewMode>,
    hull: Single<(&mut Transform, &mut LinearVelocity, &mut AngularVelocity), With<Hull>>,
) {
    active.0 = harness.model;
    view.kinematic = !harness.chain_view;
    let (mut transform, mut lin, mut ang) = hull.into_inner();
    *transform = Transform::from_xyz(0.0, HULL_REST_Y, harness.z);
    lin.0 = Vec3::ZERO;
    ang.0 = Vec3::ZERO;

    let file = File::create(&harness.out).expect("harness out path must be writable");
    let mut writer = BufWriter::new(file);
    let model_idx = MODELS.iter().position(|m| *m == harness.model).unwrap() + 1;
    writeln!(
        writer,
        "{{\"t\":\"meta\",\"model\":{model_idx},\"view\":\"{}\",\"z\":{:.3},\"warmup\":{},\"ticks\":{},\"throttle\":{:.3},\"weight\":{:.0},\"hull_rest_y\":{HULL_REST_Y},\"thickness\":{TRACK_THICKNESS}}}",
        if harness.chain_view { "chain" } else { "wrap" },
        harness.z,
        harness.warmup,
        harness.ticks,
        harness.throttle,
        HULL_MASS * 9.81,
    )
    .unwrap();

    // Field scans (meaningful for model 4; cheap and harmless otherwise). Horizontal rows: signed
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

/// Drive the scenario: zero input through warmup, then the constant scripted throttle. Runs in
/// FixedUpdate BEFORE the force systems, so phase boundaries (warmup end, `t2`) land on exact
/// ticks regardless of frame pacing — one half of the harness's bit-repeatability (the other is
/// the manual-duration clock). It overrides whatever `read_drive_input` wrote last frame.
pub(super) fn harness_drive(
    harness: Res<Harness>,
    log: Option<Res<HarnessLog>>,
    mut input: ResMut<DriveInput>,
) {
    let Some(log) = log else {
        return;
    };
    input.throttle = if log.tick < harness.warmup {
        0.0
    } else if log.tick >= harness.t2 {
        harness.throttle2
    } else {
        harness.throttle
    };
    input.steer = 0.0;
}

/// Record one `k` line per fixed tick (after the model force systems), then exit when done.
pub(super) fn harness_record(
    harness: Res<Harness>,
    log: Option<ResMut<HarnessLog>>,
    hull: Single<(&Transform, &LinearVelocity), With<Hull>>,
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
    let (transform, lin) = *hull;
    let (_, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let total: f32 = contacts.0.iter().map(|c| c.load).sum();
    let contact_rows: Vec<String> = contacts
        .0
        .iter()
        .map(|c| {
            format!(
                "[{:.4},{:.4},{:.4},{:.0},{:.3},{:.4}]",
                c.local.x, c.local.y, c.local.z, c.load, c.slip, c.normal.y
            )
        })
        .collect();
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
        "{{\"t\":\"k\",\"k\":{k},\"hull\":{},\"pitch\":{:.5},\"vel\":{},\"belt\":{},\"phase\":{},\"sup\":{:.0},\"wheels\":[{}],\"contacts\":[{}],\"chain\":[{}]}}",
        arr([
            transform.translation.x,
            transform.translation.y,
            transform.translation.z
        ]),
        pitch,
        arr([lin.0.x, lin.0.y, lin.0.z]),
        arr([belt.left, belt.right]),
        arr([phase.get(Side::Left), phase.get(Side::Right)]),
        total,
        wheel_json.join(","),
        contact_rows.join(","),
        chain_rows.join(","),
    )
    .unwrap();
    log.tick += 1;
    if log.tick >= harness.warmup + harness.ticks {
        log.writer.flush().unwrap();
        info!("harness: wrote {} ticks to {}", log.tick, harness.out);
        exit.write(AppExit::Success);
    }
}
