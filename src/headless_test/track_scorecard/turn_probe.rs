//! Turn probe (the game's tank driving physics, game state only): what the shipped tank does in
//! a turn, as per-tick counts and geometry. A diagnosis instrument, run on demand:
//!
//! `scripts/serial-cargo test --lib --features dev_ui,bitprobe turn_probe -- --ignored --nocapture`
//!
//! `TURN_PROBE_ONLY=<text>` runs only the cases whose label contains the text.
//!
//! Each case prints one `TURNPROBE {json}` line: the rig's constants, the per-tick series of the
//! run-up's last second, the steer hold and the release, and the per-station force sums over the
//! hold's closing seconds. Every series is MEASURED from Avian body state, `TrackDrive`,
//! `TankTransmission`, `TrackGripEffect` and `TrackContacts`; the `rig` block is read from the
//! rig, its transmission table and the shipped data. Every line is an observation.
//!
//! Frames: `ahead` is metres forward of the contact centre (the midpoint of the road-wheel span
//! on the hull's long axis), `right` metres to the hull's right. The rotation centre follows from
//! the hull's planar velocity `v` at that point and its yaw rate `w` about hull up (positive
//! turning left): `ahead = v_right / w`, `right = -v_forward / w`.
//!
//! Fixture: every placement, duration and bin below is fixture data.

use super::*;

/// Fixture: seconds of steer hold, of release after it, and of straight driving recorded before.
const HOLD_S: usize = 12;
const RELEASE_S: usize = 5;
const LEAD_S: usize = 1;
/// Fixture: the hold's closing seconds the station sums cover.
const WINDOW_S: usize = 3;
/// Fixture: the longest full-throttle run-up waited for a gear (s).
const RUN_UP_CAP_S: usize = 70;
/// Fixture: station bins along the hull's long axis, about the contact centre (m).
const BIN_M: f32 = 0.5;
const BINS: usize = 12;

#[derive(Clone)]
enum Ground {
    /// The card's grid at the shipped cell spacing, level.
    Flat,
    /// One level cell: two triangles under the whole world.
    FlatOneCell,
    /// The card's grid tilted by this angle (rad): rising to the hull's right at the start, or
    /// (`along`) rising ahead of it. A negative angle falls that way.
    Slope { angle: f32, along: bool },
    /// The shipped height map, hull dropped at this (x, z).
    Shipped(Vec2),
    /// Level, one layer of loose pieces under each track.
    Pieces,
}

struct Case {
    label: String,
    ground: Ground,
    /// Full-throttle run-up until this forward gear has held half a second; `None` starts at rest.
    gear: Option<u8>,
    steer: f32,
    /// Throttle during the hold and release.
    throttle: f32,
}

#[derive(Default)]
struct Series {
    cols: std::collections::BTreeMap<&'static str, Vec<f32>>,
}

impl Series {
    fn push(&mut self, key: &'static str, value: f32) {
        self.cols.entry(key).or_default().push(value);
    }
}

fn contact_center_z(geom: &RigGeom) -> (f32, f32) {
    let (lo, hi) = Side::ALL
        .iter()
        .flat_map(|&side| geom.road_wheels.get(side).iter())
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), w| {
            (lo.min(w.z), hi.max(w.z))
        });
    ((lo + hi) / 2.0, hi - lo)
}

fn transmission(bed: &Bed) -> crate::track::transmission::TransmissionState {
    bed.app
        .world()
        .get::<crate::track::sim::TankTransmission>(bed.tank)
        .expect("a tank carries its transmission")
        .0
}

/// One tick's readings into `series`; station sums into `bins` when given.
fn record(bed: &Bed, z_c: f32, series: &mut Series, bins: Option<&mut [[f32; 4]; 2 * BINS]>) {
    let hull = bed.hull();
    let world = bed.app.world();
    let (fwd, right, up) = (hull.forward(), hull.rotation * Vec3::X, hull.up());
    let com = bed.center_of_mass();
    let center = hull.position + hull.rotation * Vec3::new(0.0, 0.0, z_c);
    // Avian's linear velocity is the centre of mass's.
    let v = hull.velocity + hull.angular.cross(center - com);
    let w = hull.yaw_rate();
    series.push("w", w);
    series.push("vf", v.dot(fwd));
    series.push("vr", v.dot(right));
    series.push("x", hull.position.x);
    series.push("z", hull.position.z);
    series.push("heading", fwd.x.atan2(-fwd.z));
    series.push("pitch", hull.pitch());
    series.push("roll", hull.roll());
    let drive = world
        .get::<crate::track::sim::TrackDrive>(bed.tank)
        .expect("a tank carries its belts");
    series.push("belt_l", drive.sides[0].speed);
    series.push("belt_r", drive.sides[1].speed);
    let state = transmission(bed);
    series.push("gear", f32::from(state.gear));
    series.push("detent", f32::from(state.steer_step));
    series.push("shift", f32::from(state.shift_ticks));
    series.push("park", f32::from(u8::from(state.park)));
    series.push(
        "clutch",
        match state.clutch {
            crate::track::transmission::ClutchState::Out => 0.0,
            crate::track::transmission::ClutchState::Slip => 1.0,
            crate::track::transmission::ClutchState::In => 2.0,
        },
    );
    let effect = world
        .get::<crate::track::sim::TrackGripEffect>(bed.tank)
        .expect("a tank carries its grip effect");
    series.push("reaction_l", effect.belt_reaction[0]);
    series.push("reaction_r", effect.belt_reaction[1]);
    series.push(
        "rpm",
        state.omega_e / crate::track::transmission::RPM_TO_RAD,
    );
    let signal = world
        .get::<crate::track::sim::DriveSignal>(bed.tank)
        .expect("a tank carries its drive signal");
    series.push("steer_signal", signal.steer);
    series.push("throttle_signal", signal.throttle);

    let contacts = &world
        .get::<TrackContacts>(bed.tank)
        .expect("a driving tank reports its belt contacts")
        .0;
    let (mut count, mut load, mut load_ahead) = (0.0, 0.0, 0.0);
    let (mut lat, mut long, mut m_lat, mut m_long, mut lat_ahead) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let mut side_long = [0.0; 2];
    let mut bins = bins;
    for (si, side) in contacts.iter().enumerate() {
        for c in side {
            let arm = c.point - center;
            let ahead = arm.dot(fwd);
            let tangent = c.tangential_force();
            let (f_lat, f_long) = (tangent.dot(right), tangent.dot(fwd));
            count += 1.0;
            load += c.load;
            load_ahead += c.load * ahead;
            lat += f_lat;
            long += f_long;
            lat_ahead += f_lat * ahead;
            side_long[si] += f_long;
            m_lat += arm.cross(right * f_lat).dot(up);
            m_long += arm.cross(fwd * f_long).dot(up);
            if let Some(bins) = bins.as_deref_mut() {
                let bin = ((ahead / BIN_M + BINS as f32 / 2.0).floor() as isize)
                    .clamp(0, BINS as isize - 1) as usize;
                let row = &mut bins[si * BINS + bin];
                row[0] += c.load;
                row[1] += f_lat;
                row[2] += f_long;
                row[3] += 1.0;
            }
        }
    }
    series.push("contacts", count);
    series.push("load", load);
    series.push(
        "load_ahead",
        if load > 0.0 { load_ahead / load } else { 0.0 },
    );
    series.push("f_lat", lat);
    series.push("f_long", long);
    series.push("f_lat_moment_arm", lat_ahead);
    series.push("f_long_l", side_long[0]);
    series.push("f_long_r", side_long[1]);
    series.push("m_lat", m_lat);
    series.push("m_long", m_long);
    let (mut solver_n, mut solver_m, mut solver_points) = (0.0, 0.0, 0.0);
    for (force, _) in bed.tank_solver_forces() {
        solver_n += force.force.dot(up);
        solver_m += (force.point - center).cross(force.force).dot(up);
        solver_points += 1.0;
    }
    series.push("solver_up", solver_n);
    series.push("solver_yaw_moment", solver_m);
    series.push("solver_points", solver_points);
}

fn run(case: &Case, hold_s: usize) {
    let ground = base_ground();
    let grid = match &case.ground {
        Ground::Flat | Ground::Pieces => flat(),
        Ground::FlatOneCell => driving_plane(0.0),
        Ground::Slope { angle, along } => {
            let (tan, along) = (angle.tan(), *along);
            card_grid(move |x, z| if along { -z * tan } else { x * tan })
        }
        Ground::Shipped(_) => crate::terrain_grid::tests::shipped_grid(),
    };
    let start = match &case.ground {
        Ground::Shipped(at) => Vec3::new(at.x, grid.height_at(at.x, at.y) + 0.3, at.y),
        Ground::Slope { .. } => Vec3::ZERO,
        // Fixture: a corner of the level world, so a long run-up and a wide circle stay on it.
        Ground::Flat | Ground::FlatOneCell => Vec3::new(-150.0, 0.0, 330.0),
        Ground::Pieces => Vec3::ZERO,
    };
    let mut bed = Bed::on_ground(Some(ground), grid);
    let mut lift = 0.0;
    if matches!(case.ground, Ground::Pieces) {
        lift = pile_under(&mut bed);
    }
    let normal = match &case.ground {
        Ground::Slope { angle, along: true } => Vec3::new(0.0, 1.0, angle.tan()),
        Ground::Slope {
            angle,
            along: false,
        } => Vec3::new(-angle.tan(), 1.0, 0.0),
        _ => Vec3::Y,
    };
    bed.place(
        start + Vec3::Y * lift,
        Quat::from_rotation_arc(Vec3::Y, normal.normalize()),
    );
    bed.start();
    for _ in 0..2 * TICKS {
        bed.tick(0.0, 0.0);
    }

    let geom = bed.geom().clone();
    let (z_c, span) = contact_center_z(&geom);
    let trans = bed
        .app
        .world()
        .resource::<crate::track::sim::TrackGear>()
        .trans()
        .clone();
    let com_local = bed
        .app
        .world()
        .get::<ComputedCenterOfMass>(bed.tank)
        .expect("tank centre of mass")
        .0;
    // The orbit camera's anchor (`camera::capture_turret_pivot`): the turret's place in the hull.
    let turret_local = {
        let world = bed.app.world();
        let rig = world
            .get::<crate::tank::Rig>(bed.tank)
            .expect("a tank carries its rig");
        let (mut at, mut node) = (Vec3::ZERO, rig.turret);
        while node != bed.tank {
            let local = world
                .get::<Transform>(node)
                .expect("a rig node's transform");
            at = local.transform_point(at);
            node = world
                .get::<ChildOf>(node)
                .expect("a rig node's parent")
                .parent();
        }
        at
    };
    let loop_z = Side::ALL
        .iter()
        .flat_map(|&side| geom.rest.get(side).iter())
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &(c, r)| {
            (lo.min(c.x - r), hi.max(c.x + r))
        });

    let mut series = Series::default();
    let mut bins = [[0.0; 4]; 2 * BINS];
    let mut run_up_ticks = 0;
    if let Some(gear) = case.gear {
        // The scheduler holds a gear only briefly at full throttle, so the hold starts on the
        // first moving tick in the target gear.
        loop {
            bed.tick(1.0, 0.0);
            record(&bed, z_c, &mut series, None);
            run_up_ticks += 1;
            let state = transmission(&bed);
            if (state.gear >= gear && !state.reverse && bed.hull().speed() > 0.3)
                || run_up_ticks >= RUN_UP_CAP_S * TICKS
            {
                break;
            }
        }
    }
    let lead_throttle = if case.gear.is_some() { 1.0 } else { 0.0 };
    while run_up_ticks < LEAD_S * TICKS {
        bed.tick(lead_throttle, 0.0);
        record(&bed, z_c, &mut series, None);
        run_up_ticks += 1;
    }
    for col in series.cols.values_mut() {
        col.drain(..col.len() - LEAD_S * TICKS);
    }
    for tick in 0..hold_s * TICKS {
        bed.tick(case.throttle, case.steer);
        let windowed = tick >= (hold_s - WINDOW_S) * TICKS;
        record(&bed, z_c, &mut series, windowed.then_some(&mut bins));
    }
    for _ in 0..RELEASE_S * TICKS {
        bed.tick(case.throttle, 0.0);
        record(&bed, z_c, &mut series, None);
    }

    let window = (WINDOW_S * TICKS) as f32;
    let bins: Vec<Value> = (0..BINS)
        .map(|bin| {
            let ahead = (bin as f32 - BINS as f32 / 2.0 + 0.5) * BIN_M;
            let (l, r) = (bins[bin], bins[BINS + bin]);
            json!({
                "ahead_m": ahead,
                "left": l.map(|v| v / window),
                "right": r.map(|v| v / window),
            })
        })
        .collect();
    let line = json!({
        "label": case.label,
        "steer": case.steer,
        "throttle": case.throttle,
        "target_gear": case.gear,
        "run_up_ticks": run_up_ticks,
        "lead_ticks": LEAD_S * TICKS,
        "hold_ticks": hold_s * TICKS,
        "release_ticks": RELEASE_S * TICKS,
        "window_ticks": WINDOW_S * TICKS,
        "tick_hz": TICKS,
        "rig": {
            "contact_center_z": z_c,
            "wheel_span_m": span,
            "half_tread_m": geom.plane_x,
            "plane_x": [geom.side_plane_x(Side::Left), geom.side_plane_x(Side::Right)],
            "com_local": com_local.to_array(),
            "turret_local": turret_local.to_array(),
            "shear_floor_m": crate::track::forces::GRIP_SHEAR_FLOOR_M,
            "loop_z": [loop_z.0, loop_z.1],
            "mass_kg": bed.mass(),
            "grip": ground.grip,
            "rolling": ground.rolling_resistance,
            "radii": trans.steer_radii_m,
            "neutral_d_full": trans.neutral_d_full,
            "wide_on": crate::track::drive::drive_feel().wide_on.0,
            "tight_on": crate::track::drive::drive_feel().tight_on.0,
            "rise_per_second": crate::track::drive::drive_feel().rise_per_second.0,
        },
        "bins": bins,
        "bin_fields": ["load_n", "f_right_n", "f_forward_n", "contacts"],
        "series": series.cols,
    });
    println!("TURNPROBE {line}");
}

/// The shipped map's ground under candidate drops: (x, z, mean slope rad, roughness m), where
/// roughness is the RMS residual of heights about their best plane over a 40 m square.
fn shipped_sites() -> Vec<(Vec2, f32, f32)> {
    let grid = crate::terrain_grid::tests::shipped_grid();
    let half = grid.half_extent();
    let mut sites = Vec::new();
    let mut x = -half + 100.0;
    while x < half - 100.0 {
        let mut z = -half + 100.0;
        while z < half - 100.0 {
            let (mut n, mut sh) = (0.0, 0.0);
            let (mut sxx, mut szz, mut sxh, mut szh) = (0.0, 0.0, 0.0, 0.0);
            let mut samples = Vec::new();
            for i in -10..=10 {
                for k in -10..=10 {
                    let (dx, dz) = (i as f32 * 2.0, k as f32 * 2.0);
                    let h = grid.height_at(x + dx, z + dz);
                    n += 1.0;
                    sh += h;
                    sxx += dx * dx;
                    szz += dz * dz;
                    sxh += dx * h;
                    szh += dz * h;
                    samples.push((dx, dz, h));
                }
            }
            // The lattice is symmetric, so the plane fit separates per axis.
            let (gx, gz, mean) = (sxh / sxx, szh / szz, sh / n);
            let rough = (samples
                .iter()
                .map(|&(dx, dz, h)| (h - mean - gx * dx - gz * dz).powi(2))
                .sum::<f32>()
                / n)
                .sqrt();
            sites.push((Vec2::new(x, z), gx.hypot(gz).atan(), rough));
            z += 50.0;
        }
        x += 50.0;
    }
    sites
}

fn flat_case(gear: Option<u8>, steer: f32, name: &str) -> Case {
    Case {
        label: format!("flat {name}"),
        ground: Ground::Flat,
        gear,
        steer,
        throttle: if gear.is_some() { 1.0 } else { 0.0 },
    }
}

/// Fixture: one notch of lever is the wide detent, the full lever the tight one.
fn detents() -> [(f32, &'static str); 2] {
    let feel = crate::track::drive::drive_feel();
    [(feel.notch_step(), "wide"), (1.0, "tight")]
}

fn selected(label: &str) -> bool {
    std::env::var("TURN_PROBE_ONLY").map_or(true, |only| label.contains(&only))
}

#[test]
#[ignore = "the turn probe, run on demand"]
fn turn_probe() {
    let mut cases = vec![flat_case(None, 1.0, "neutral pivot")];
    let gears = Bed::new(flat())
        .app
        .world()
        .resource::<crate::track::sim::TrackGear>()
        .trans()
        .steer_radii_m
        .len() as u8;
    for gear in 1..=gears {
        for (steer, name) in detents() {
            cases.push(flat_case(Some(gear), steer, &format!("g{gear} {name}")));
        }
    }
    // Mirror, coasting and lifted-throttle turns at two gears.
    for gear in [3, 5] {
        cases.push(flat_case(
            Some(gear),
            -1.0,
            &format!("g{gear} mirrored tight"),
        ));
        cases.push(Case {
            throttle: 0.0,
            ..flat_case(Some(gear), 1.0, &format!("g{gear} lifted tight"))
        });
    }
    // Part lever travel at low gears.
    for gear in [1, 2, 3] {
        cases.push(Case {
            throttle: 1.0 / 3.0,
            ..flat_case(Some(gear), 1.0, &format!("g{gear} third-throttle tight"))
        });
    }
    for (gear, steer, turn) in [
        (2, 1.0, "g2 tight"),
        (4, 1.0, "g4 tight"),
        (4, 1.0 / 3.0, "g4 wide"),
    ] {
        cases.push(Case {
            label: format!("one-cell {turn}"),
            ground: Ground::FlatOneCell,
            gear: Some(gear),
            steer,
            throttle: 1.0,
        });
    }
    cases.push(Case {
        label: "one-cell neutral pivot".into(),
        ground: Ground::FlatOneCell,
        gear: None,
        steer: 1.0,
        throttle: 0.0,
    });
    for degrees in [2.0_f32, 5.0, 10.0] {
        for (along, name) in [(false, "across"), (true, "uphill")] {
            let ground = Ground::Slope {
                angle: degrees.to_radians(),
                along,
            };
            for steer in [1.0, -1.0] {
                cases.push(Case {
                    label: format!("slope{degrees} {name} neutral pivot steer{steer}"),
                    ground: ground.clone(),
                    gear: None,
                    steer,
                    throttle: 0.0,
                });
            }
            if degrees == 5.0 {
                for (gear, steer, turn) in [
                    (1, 1.0, "g1 tight"),
                    (2, 1.0, "g2 tight"),
                    (4, 1.0 / 3.0, "g4 wide"),
                ] {
                    cases.push(Case {
                        label: format!("slope{degrees} {name} {turn}"),
                        ground: ground.clone(),
                        gear: Some(gear),
                        steer,
                        throttle: 1.0,
                    });
                }
            }
        }
    }
    cases.push(Case {
        label: "pieces neutral pivot".into(),
        ground: Ground::Pieces,
        gear: None,
        steer: 1.0,
        throttle: 0.0,
    });

    // Fixture: the gentlest site with a slope of at least 3 degrees, and the roughest site under
    // 8 degrees of mean slope.
    let sites = shipped_sites();
    let gentle = sites
        .iter()
        .filter(|s| s.1 >= 3f32.to_radians() && s.1 <= 6f32.to_radians())
        .min_by(|a, b| a.2.total_cmp(&b.2));
    let rough = sites
        .iter()
        .filter(|s| s.1 <= 8f32.to_radians())
        .max_by(|a, b| a.2.total_cmp(&b.2));
    for (site, name) in [(gentle, "shipped-gentle"), (rough, "shipped-rough")] {
        let Some(&(at, slope, roughness)) = site else {
            println!("TURNPROBE-NOTE no {name} site on the shipped map");
            continue;
        };
        println!(
            "TURNPROBE-NOTE {name} site x={} z={} slope_deg={} roughness_m={}",
            at.x,
            at.y,
            slope.to_degrees(),
            roughness
        );
        for (gear, steer, turn) in [
            (2, 1.0, "g2 tight"),
            (4, 1.0, "g4 tight"),
            (4, 1.0 / 3.0, "g4 wide"),
        ] {
            cases.push(Case {
                label: format!("{name} {turn}"),
                ground: Ground::Shipped(at),
                gear: Some(gear),
                steer,
                throttle: 1.0,
            });
        }
        cases.push(Case {
            label: format!("{name} neutral pivot"),
            ground: Ground::Shipped(at),
            gear: None,
            steer: 1.0,
            throttle: 0.0,
        });
    }

    for case in cases.iter().filter(|case| selected(&case.label)) {
        run(case, HOLD_S);
    }
}

/// Fixture: a full-steer neutral pivot's hold, long enough for one full turn (s).
const PIVOT_TURN_S: usize = 64;

/// Full-steer neutral pivots for one full turn, level and on slopes facing up, down and across.
/// `TURN_PROBE_ONLY` narrows these too.
#[test]
#[ignore = "the turn probe, run on demand"]
fn pivot_probe() {
    let mut cases = vec![Case {
        label: "pivot flat".into(),
        ..flat_case(None, 1.0, "neutral pivot")
    }];
    // Fixture: slopes in degrees; the steepest exceed the hull's longitudinal holds.
    for degrees in [5.0_f32, 10.0, 15.0, 25.0, 35.0] {
        for (angle, along, facing) in [
            (degrees, true, "uphill"),
            (-degrees, true, "downhill"),
            (degrees, false, "across"),
        ] {
            cases.push(Case {
                label: format!("pivot slope{degrees} {facing}"),
                ground: Ground::Slope {
                    angle: angle.to_radians(),
                    along,
                },
                gear: None,
                steer: 1.0,
                throttle: 0.0,
            });
        }
    }
    for case in cases.iter().filter(|case| selected(&case.label)) {
        run(case, PIVOT_TURN_S);
    }
}
