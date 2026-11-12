//! Belt friction probe (the game's tank driving physics, game state only): how the solver's
//! friction on a moving shoe face and the belt's speed answer each other tick by tick on loose
//! pieces. A diagnosis instrument, run on demand:
//!
//! `scripts/serial-cargo test --lib --features dev_ui,bitprobe belt_friction_probe -- --ignored --nocapture`
//!
//! `BELT_FRICTION_ONLY=<text>` runs only the cases whose label contains the text and refuses a
//! text that selects none; `BELT_FRICTION_TAG=<text>` names the run in the per-tick CSV files
//! under `target/belt-friction/`.
//!
//! Each case prints one `BELTFRICTION {json}` line. Per clutch regime, each taken within runs of
//! consecutive ticks: the tick-to-tick alternation of the friction the solver delivered to each
//! belt, of the belt's speed and of the hull's, and how often that friction flips sign. Every value
//! is MEASURED from `ContactTransfers` (read through `stop_contacts`, as the drivetrain reads it),
//! `TrackDrive`, `TankTransmission`, `TrackGripEffect` and Avian body state, except the `rig`
//! block: the belt inertia is the drivetrain's construction and the mode masses are DERIVED from
//! Avian's mass properties.
//!
//! Fixture: every placement, duration and threshold below is fixture data.

use std::ops::Range;

use super::turn_probe::alternation;
use super::*;
use crate::track::transmission::ClutchState;

/// Fixture: seconds of a case's drive.
const DRIVE_S: usize = 4;
/// Fixture: a friction reading counts toward a sign flip only above this share of its grip limit.
const FLIP_SHARE: f32 = 0.1;

/// One side's solver contact over the step a tick just ran.
#[derive(Clone, Copy, Default)]
struct SideRead {
    /// The belt's reaction to the friction the solver delivered over the step (N, along travel).
    delivered: f32,
    /// That friction's Coulomb limit (N).
    grip: f32,
    /// The reaction the belt landed against this tick (N): `TrackGripEffect`.
    charged: f32,
}

struct Reading {
    clutch: ClutchState,
    gear: u8,
    shift: u8,
    belt: [f32; 2],
    hull_speed: f32,
    yaw_rate: f32,
    sides: [SideRead; 2],
    /// The transmission's last solve this tick.
    drive: crate::bitprobe::TransmissionProbe,
}

impl Reading {
    fn finite(&self) -> bool {
        let sides = self
            .sides
            .iter()
            .flat_map(|side| [side.delivered, side.grip, side.charged]);
        self.belt
            .into_iter()
            .chain([self.hull_speed, self.yaw_rate])
            .chain(sides)
            .all(f32::is_finite)
    }
}

/// The step just run, read as the drivetrain reads it, plus the contact kinematics.
fn read(bed: &mut Bed) -> Reading {
    let tank = bed.tank;
    let hull = bed.hull();
    let mut sides = bed.hard_reactions().map(|hard| {
        hard.map_or(SideRead::default(), |h| SideRead {
            delivered: h.belt,
            grip: h.grip,
            ..default()
        })
    });
    let world = bed.app.world();
    let effect = world
        .get::<crate::track::sim::TrackGripEffect>(tank)
        .expect("a driven tank publishes its belt reaction");
    for (side, charged) in sides.iter_mut().zip(effect.belt_reaction) {
        side.charged = charged;
    }
    let drive = world
        .get::<crate::track::sim::TrackDrive>(tank)
        .expect("a tank carries its belts");
    let state = world
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("a tank carries its transmission")
        .0;
    Reading {
        clutch: state.clutch,
        gear: state.gear,
        shift: state.shift_ticks,
        belt: [drive.sides[0].speed, drive.sides[1].speed],
        hull_speed: hull.speed(),
        yaw_rate: hull.yaw_rate(),
        sides,
        drive: effect.transmission_probe,
    }
}

/// The runs of consecutive ticks in one clutch regime.
fn runs(ticks: &[Reading], clutch: ClutchState) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    for (i, t) in ticks.iter().enumerate() {
        if t.clutch != clutch {
            continue;
        }
        match runs.last_mut() {
            Some(run) if run.end == i => run.end = i + 1,
            _ => runs.push(i..i + 1),
        }
    }
    runs
}

/// A series' alternation and its largest change between two ticks, each taken within a run and
/// then over the runs of three ticks or more: a regime change is no oscillation.
fn within(ticks: &[Reading], runs: &[Range<usize>], key: impl Fn(&Reading) -> f32) -> Value {
    let (mut weighted, mut windows, mut largest) = (0.0, 0, 0.0_f32);
    for run in runs.iter().filter(|run| run.len() >= 3) {
        let x: Vec<f32> = ticks[run.clone()].iter().map(&key).collect();
        let amplitude = alternation(&x).expect("a run of three ticks has an alternation");
        weighted += amplitude * (x.len() - 2) as f32;
        windows += x.len() - 2;
        largest = x
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(largest, f32::max);
    }
    if windows == 0 {
        return Value::Null;
    }
    json!({"amplitude": weighted / windows as f32, "largest_step": largest})
}

/// Ticks whose delivered friction reverses against the tick before, both above the flip share of
/// their grip.
fn flips(run: &[Reading], si: usize) -> usize {
    run.windows(2)
        .filter(|w| {
            let (a, b) = (w[0].sides[si], w[1].sides[si]);
            a.delivered * b.delivered < 0.0
                && a.delivered.abs() > FLIP_SHARE * a.grip
                && b.delivered.abs() > FLIP_SHARE * b.grip
        })
        .count()
}

/// The readings of one clutch regime.
fn regime(ticks: &[Reading], clutch: ClutchState) -> Value {
    let runs = runs(ticks, clutch);
    let all: Vec<&Reading> = runs.iter().flat_map(|run| &ticks[run.clone()]).collect();
    if all.is_empty() {
        return Value::Null;
    }
    let per_side = |si: usize| {
        let n = all.len() as f32;
        json!({
            "delivered_kn": within(ticks, &runs, |t| t.sides[si].delivered / 1e3),
            "belt_m_s": within(ticks, &runs, |t| t.belt[si]),
            "flips": runs.iter().map(|run| flips(&ticks[run.clone()], si)).sum::<usize>(),
            "saturated_share": all.iter().filter(|t| {
                let s = t.sides[si];
                s.grip > 0.0 && s.delivered.abs() >= 0.95 * s.grip
            }).count() as f32 / n,
            "mean_grip_kn": all.iter().map(|t| t.sides[si].grip).sum::<f32>() / n / 1e3,
        })
    };
    json!({
        "ticks": all.len(),
        "runs": runs.len(),
        "hull_m_s": within(ticks, &runs, |t| t.hull_speed),
        "yaw_rate": within(ticks, &runs, |t| t.yaw_rate),
        "total_delivered_kn": within(ticks, &runs, |t| {
            (t.sides[0].delivered + t.sides[1].delivered) / 1e3
        }),
        "left": per_side(0),
        "right": per_side(1),
    })
}

fn write_series(tag: &str, label: &str, ticks: &[Reading]) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/belt-friction");
    std::fs::create_dir_all(&dir).expect("the target directory exists or can be made");
    let mut csv = String::from(
        "tick,clutch,gear,shift,hull,yaw,belt_l,belt_r,deliv_l,deliv_r,charged_l,charged_r,\
         grip_l,grip_r,sprocket_l,sprocket_r,clutch_n,tie_n,tie_l,power_scale,react_in_l,\
         react_in_r\n",
    );
    for (i, t) in ticks.iter().enumerate() {
        let [l, r] = t.sides;
        let d = &t.drive;
        csv += &format!(
            "{i},{:?},{},{},{:.4},{:.4},{:.4},{:.4},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},\
             {:.0},{:.0},{:.3},{:.3},{:.0},{:.0}\n",
            t.clutch,
            t.gear,
            t.shift,
            t.hull_speed,
            t.yaw_rate,
            t.belt[0],
            t.belt[1],
            l.delivered,
            r.delivered,
            l.charged,
            r.charged,
            l.grip,
            r.grip,
            d.forces[0],
            d.forces[1],
            d.f_c_pre_scale,
            d.lambda,
            d.j[0],
            d.power_scale,
            d.reactions[0],
            d.reactions[1],
        );
    }
    std::fs::write(dir.join(format!("{tag}-{label}.csv")), csv).expect("the series writes");
}

/// The hull's mobility along each belt at the last step's solver contacts, from the grip-weighted
/// mean direction and arm per side (DERIVED from Avian's mass properties): each side's mass with
/// the other free, a side's share in the belts' common and opposed modes, and each side's mass with
/// the other's slip held. `Null` until both belts bear on the solver's contacts.
fn mode_masses(bed: &Bed) -> Value {
    use crate::track::stop_contacts::BeltFace;
    let world = bed.app.world();
    let hull = bed.hull();
    let com = hull.position
        + hull.rotation
            * world
                .get::<ComputedCenterOfMass>(bed.tank)
                .expect("the tank's centre of mass")
                .0;
    let inverse_mass = world
        .get::<ComputedMass>(bed.tank)
        .expect("the tank's mass")
        .inverse();
    let inverse_inertia = world
        .get::<ComputedAngularInertia>(bed.tank)
        .expect("the tank's inertia")
        .rotated(hull.rotation)
        .inverse()
        .to_mat3();
    let mut sums = [(0.0_f32, Vec3::ZERO, Vec3::ZERO); 2];
    for (point, _) in bed.solver_points() {
        let other = point
            .other_collider
            .expect("a solver constraint joins two colliders");
        for (collider, sign) in [(point.reference_collider, -1.0), (other, 1.0)] {
            let Some(face) = world.get::<BeltFace>(collider) else {
                continue; // Not a moving shoe face.
            };
            let owner = world
                .get::<ColliderOf>(collider)
                .expect("a face hangs on a body");
            if owner.body != bed.tank || point.impulse == Vec3::ZERO {
                continue;
            }
            let outward = -sign * point.normal;
            let direction = hull.rotation
                * face.direction(
                    hull.rotation.inverse() * (point.point - hull.position),
                    hull.rotation.inverse() * outward,
                );
            let weight = point.friction * (sign * point.impulse).dot(outward).abs();
            let side = &mut sums[face.side.index()];
            side.0 += weight;
            side.1 += weight * direction;
            side.2 += weight * (point.point - com).cross(direction);
        }
    }
    if sums.iter().any(|side| side.0 == 0.0) {
        return Value::Null;
    }
    let [l, r] = sums.map(|(weight, along, arm)| (along / weight, arm / weight));
    let w = |a: (Vec3, Vec3), b: (Vec3, Vec3)| {
        inverse_mass * a.0.dot(b.0) + a.1.dot(inverse_inertia * b.1)
    };
    let (w11, w22, w12) = (w(l, l), w(r, r), w(l, r));
    let mean = (w11 + w22) / 2.0;
    let det = w11 * w22 - w12 * w12;
    json!({
        "single_kg": [1.0 / w11, 1.0 / w22],
        "common_kg": 1.0 / (mean + w12),
        "opposed_kg": 1.0 / (mean - w12),
        "other_held_kg": [w22 / det, w11 / det],
    })
}

/// A probe case: its fixture, built when the case runs, and the drive it holds.
struct Case {
    label: &'static str,
    bed: fn() -> Bed,
    seconds: usize,
    throttle: f32,
    steer: f32,
}

const CASES: &[Case] = &[
    Case {
        label: "slab-pull",
        bed: slab_bed,
        seconds: DRIVE_S,
        throttle: 1.0,
        steer: 0.0,
    },
    Case {
        label: "slab-pivot",
        bed: slab_bed,
        seconds: DRIVE_S,
        throttle: 0.0,
        steer: 1.0,
    },
    Case {
        label: "pieces-pull",
        bed: pieces_bed,
        seconds: DRIVE_S,
        throttle: 1.0,
        steer: 0.0,
    },
    Case {
        label: "pieces-pivot",
        bed: pieces_bed,
        seconds: DRIVE_S,
        throttle: 0.0,
        steer: 1.0,
    },
    #[cfg(feature = "dev_ui")]
    Case {
        label: "house-pivot",
        bed: house_bed,
        seconds: 2 * DRIVE_S,
        throttle: 0.0,
        steer: 1.0,
    },
];

fn run(case: &Case, tag: &str) {
    let mut bed = (case.bed)();
    let (mut ticks, mut modes) = (Vec::new(), Value::Null);
    for _ in 0..case.seconds * TICKS {
        bed.tick(case.throttle, case.steer);
        ticks.push(read(&mut bed));
        if modes.is_null() {
            modes = mode_masses(&bed);
        }
    }
    let label = case.label;
    assert!(
        ticks.iter().all(Reading::finite),
        "{label}: a reading is not finite"
    );
    assert!(
        !modes.is_null(),
        "{label}: no step had both belts bearing on the solver's contacts"
    );
    write_series(tag, label, &ticks);
    let line = json!({
        "label": label,
        "tag": tag,
        "throttle": case.throttle,
        "steer": case.steer,
        "tick_hz": TICKS,
        "rig": {
            "belt_inertia_kg": bed
                .app
                .world()
                .resource::<crate::track::sim::TrackGear>()
                .construction()
                .belt_inertia,
            "hull_mass_kg": bed.mass(),
            "modes_at_first_contact": modes,
        },
        "out": regime(&ticks, ClutchState::Out),
        "slip": regime(&ticks, ClutchState::Slip),
        "in": regime(&ticks, ClutchState::In),
        "end_hull_m_s": ticks.last().map(|t| t.hull_speed),
    });
    println!("BELTFRICTION {line}");
}

/// One layer of pieces under each track (`pile_under`), settled.
fn pieces_bed() -> Bed {
    let mut bed = Bed::on_ground(Some(base_ground()), flat());
    let lift = pile_under(&mut bed);
    bed.place(Vec3::Y * lift, Quat::IDENTITY);
    bed.start();
    for _ in 0..2 * TICKS {
        bed.tick(0.0, 0.0);
    }
    bed
}

/// The construction fixture's house dropped into a heap, the tank driven onto its middle at
/// full throttle, braked until the transmission parks and left to rest a second there.
#[cfg(feature = "dev_ui")]
fn house_bed() -> Bed {
    use super::rubble_climb::{Case, Heap, Mode, live_pieces, settle};
    let case = Case {
        label: "house".into(),
        heap: Heap::House,
        throttle: 1.0,
        offset: 0.0,
        mode: Mode::Through,
    };
    let (mut bed, site) = settle(&case);
    let middle = (site.near + site.far) / 2.0;
    let mut ticks = 0;
    while bed.hull().position.z > middle && ticks < 10 * TICKS {
        bed.tick(1.0, 0.0);
        bed.pieces = live_pieces(&mut bed);
        ticks += 1;
    }
    assert!(
        bed.hull().position.z <= middle,
        "the tank did not reach the heap's middle in ten seconds"
    );
    let parked = |bed: &Bed| {
        bed.app
            .world()
            .get::<crate::track::sim::TankTransmission>(bed.tank)
            .expect("a tank carries its transmission")
            .0
            .park
    };
    let mut ticks = 0;
    while !parked(&bed) && ticks < 10 * TICKS {
        bed.tick(-1.0, 0.0);
        ticks += 1;
    }
    assert!(
        parked(&bed),
        "the transmission did not park in ten seconds of braking"
    );
    let (mut rested, mut ticks) = (0, 0);
    while rested < TICKS && ticks < 15 * TICKS {
        bed.tick(0.0, 0.0);
        rested = if bed.hull().speed().abs() < 0.05 {
            rested + 1
        } else {
            0
        };
        ticks += 1;
    }
    assert!(
        rested == TICKS,
        "the hull did not rest a second within fifteen"
    );
    bed
}

#[test]
#[ignore = "the belt friction probe, run on demand"]
fn belt_friction_probe() {
    let lever = |name: &str| crate::env_text(name).unwrap_or_else(|error| panic!("{error}"));
    let only = lever("BELT_FRICTION_ONLY");
    let tag = lever("BELT_FRICTION_TAG").unwrap_or_else(|| "run".to_owned());
    let selected: Vec<&Case> = CASES
        .iter()
        .filter(|case| only.as_ref().is_none_or(|only| case.label.contains(only)))
        .collect();
    assert!(
        !selected.is_empty(),
        "BELT_FRICTION_ONLY={only:?} selects none of {:?}",
        CASES.iter().map(|case| case.label).collect::<Vec<_>>()
    );
    for case in selected {
        run(case, &tag);
    }
}
