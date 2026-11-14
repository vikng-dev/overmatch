//! Creep stall probe (the game's tank driving model, game state only): the shipped tank's every
//! tick while it climbs at a steady pedal, on the settled house heap and on plain ground of a fixed
//! grade. A diagnosis instrument, run on demand:
//!
//! `scripts/serial-cargo test --lib --features dev_ui,bitprobe creep_stall_probe -- --ignored --nocapture`
//!
//! `CREEP_STALL_CASE=heap` (the default), `slope<degrees>` or `turn` (flat ground, the wide
//! detent held) picks the drive;
//! `CREEP_STALL_SEED=<n>` the start, shifted by [`nudge`] (default 0);
//! `CREEP_STALL_THROTTLE=<pedal>` the steady request (default 0.3);
//! `CREEP_STALL_TAG=<text>` names the per-tick CSV under `target/creep-stall/`.
//!
//! Each run prints one `CREEPSTALL {json}` line, its gear changes among it, and writes one row per
//! tick: the hull, the drivetrain after the tick, its trace
//! ([`crate::bitprobe::TransmissionProbe`]), the tick's forces and what the Driver read. Every
//! value is MEASURED from that state; every line is an observation.
//!
//! Fixture: every duration and placement below is fixture data.

use super::rubble_climb::{APPROACH_M, Case, Heap, Mode, ROLES, settle};
use super::*;

/// Fixture: the drive's length (s).
const DRIVE_S: usize = 40;
/// Fixture: the rest between placing the tank and the drive (s).
const REST_S: usize = 2;

/// Seed `seed`'s shift of the tank's start: none for seed 0, else sideways and along each within
/// half a millimetre, spread by the golden-ratio sequences. It moves the start against the ground
/// and is read by no law.
fn nudge(seed: u32) -> Vec3 {
    let spread = |step: f64| ((f64::from(seed) * step).fract() - 0.5) as f32 * 1e-3;
    match seed {
        0 => Vec3::ZERO,
        _ => Vec3::new(spread(0.618_034), 0.0, spread(0.754_878)),
    }
}

enum Ground {
    Heap,
    /// A plane of this grade (rad), the tank facing uphill.
    Slope(f32),
}

/// The bed at rest on its ground, the hull's start and its forward axis there, and the heap's
/// near and far edges along −z where there is one (m).
fn prepare(ground: &Ground, seed: u32, throttle: f32) -> (Bed, Vec3, Vec3, Option<(f32, f32)>) {
    let shift = nudge(seed);
    match *ground {
        Ground::Heap => {
            let (bed, site) = settle(&Case {
                label: String::new(),
                heap: Heap::House,
                throttle,
                offset: shift.x,
                approach_m: APPROACH_M + shift.z,
                mode: Mode::Through,
            });
            let hull = bed.hull();
            (
                bed,
                hull.position,
                hull.forward(),
                Some((site.near, site.far)),
            )
        }
        Ground::Slope(angle) => {
            let mut bed = Bed::on_ground(Some(base_ground()), card_grid(|_, z| -z * angle.tan()));
            let rotation = Quat::from_rotation_x(angle);
            bed.place(shift, rotation);
            bed.start();
            for _ in 0..REST_S * TICKS {
                bed.tick(0.0, 0.0);
            }
            let hull = bed.hull();
            (bed, hull.position, rotation * Vec3::NEG_Z, None)
        }
    }
}

/// The tick's forces along `forward` (N): what the belts met, sampled and solver, and what reached
/// the hull. The belts' reaction is the drive's load along each contact's travel; the push is
/// what the Driver's ledger reads ([`crate::track::driver`]); the solver's forces on the hull by
/// the tank collider they act on, and their upward sum.
fn loads(bed: &mut Bed, forward: Vec3) -> String {
    let hard = bed.hard_reactions();
    let world = bed.app.world();
    let effect = world
        .get::<crate::track::sim::TrackGripEffect>(bed.tank)
        .expect("a driven tank carries its grip effect");
    let contacts = &world
        .get::<TrackContacts>(bed.tank)
        .expect("a driving tank reports its belt contacts")
        .0;
    let sampled = contacts.iter().flatten();
    let soft_belt: f32 = sampled.clone().map(|c| c.f_long).sum();
    let soft_push: f32 = sampled.clone().map(|c| c.traction.dot(forward)).sum();
    let soft_normal: f32 = sampled.map(|c| (c.normal * c.load).dot(forward)).sum();
    let hard = hard.iter().flatten();
    let hard_belt: f32 = hard.clone().map(|h| h.belt).sum();
    let hard_push: f32 = hard.map(|h| h.friction.dot(forward)).sum();
    let mut by_role = [0.0_f32; 4];
    let mut up = 0.0;
    for (point, force) in bed.solver_points() {
        let (collider, force) = if point.other == bed.tank {
            (
                point
                    .other_collider
                    .expect("a solver contact names both of its colliders"),
                force,
            )
        } else if point.reference == bed.tank {
            (point.reference_collider, -force)
        } else {
            continue;
        };
        let role = bed.role(collider);
        let index = ROLES
            .iter()
            .position(|r| role.starts_with(r))
            .unwrap_or_else(|| panic!("an unclassified tank collider met a body: {role}"));
        by_role[index] += force.dot(forward);
        up += force.y;
    }
    format!(
        ",{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0},{:.0}",
        effect.belt_reaction[0] + effect.belt_reaction[1],
        effect.tangential_force.dot(forward),
        soft_belt,
        soft_push,
        soft_normal,
        hard_belt,
        hard_push,
        by_role[0],
        by_role[1],
        by_role[2],
        by_role[3],
        up,
    )
}

/// One tick's CSV row: the hull, the drivetrain after the tick, its trace, the tick's forces
/// ([`loads`]) and the Driver's reading this tick, empty before it has one.
fn row(bed: &mut Bed, begin: Vec3, forward: Vec3) -> String {
    let forces = loads(bed, forward);
    let hull = bed.hull();
    let world = bed.app.world();
    let state = world
        .get::<crate::track::sim::TankTransmission>(bed.tank)
        .expect("a tank carries its transmission")
        .0;
    let drive = world
        .get::<crate::track::sim::TrackDrive>(bed.tank)
        .expect("a tank carries its belts");
    let trace = world
        .get::<crate::track::sim::TrackGripEffect>(bed.tank)
        .expect("a driven tank carries its grip effect")
        .transmission_probe;
    let reading = world
        .get::<crate::track::driver::DriverSense>(bed.tank)
        .expect("a driving tank carries its Driver's ledger")
        .reading;
    format!(
        "{:.4},{:.4},{:.4},{:.3},{:.4},{},{:?},{},{},{:.1},{:.1},{:.0},{:.3},{},{:.3}{forces},{},{}",
        (hull.position - begin).dot(forward),
        hull.position.y - begin.y,
        hull.speed(),
        hull.pitch().to_degrees(),
        (drive.sides[0].speed + drive.sides[1].speed) / 2.0,
        state.gear,
        state.clutch,
        state.shift_ticks,
        u8::from(state.park),
        state.omega_e / crate::track::transmission::RPM_TO_RAD,
        trace.tau_c,
        trace.delivered_force_n,
        trace.u_fuel,
        u8::from(trace.engaged),
        trace.power_scale,
        reading.map_or(String::new(), |s| format!("{:.4}", s.speed)),
        reading.map_or(String::new(), |s| format!("{:.0}", s.resistance)),
    )
}

#[test]
#[ignore = "the creep stall probe, run on demand"]
fn creep_stall_probe() {
    let case = std::env::var("CREEP_STALL_CASE").unwrap_or_else(|_| "heap".to_owned());
    let seed: u32 = std::env::var("CREEP_STALL_SEED").map_or(0, |s| {
        s.parse().expect("CREEP_STALL_SEED is a whole number")
    });
    let throttle: f32 = std::env::var("CREEP_STALL_THROTTLE").map_or(0.3, |s| {
        s.parse().expect("CREEP_STALL_THROTTLE is a number")
    });
    let tag = std::env::var("CREEP_STALL_TAG").unwrap_or_else(|_| "run".to_owned());
    let (ground, steer) = match case.strip_prefix("slope") {
        Some(deg) => (
            Ground::Slope(
                deg.parse::<f32>()
                    .expect("a slope case names its grade in degrees")
                    .to_radians(),
            ),
            0.0,
        ),
        None if case == "heap" => (Ground::Heap, 0.0),
        None if case == "turn" => {
            let feel = crate::track::drive::drive_feel();
            (Ground::Slope(0.0), 0.5 * (feel.wide_on.0 + feel.tight_on.0))
        }
        None => panic!("CREEP_STALL_CASE is heap, turn or slope<degrees>, not {case}"),
    };
    let (mut bed, begin, forward, heap) = prepare(&ground, seed, throttle);
    let mut csv = String::from(
        "tick,travel,rise,speed,pitch_deg,belt,gear,clutch,shift,park,rpm,tau_c,delivered,\
         u_fuel,engaged,power_scale,belt_reaction,grip_fwd,soft_belt,soft_push,soft_normal,\
         hard_belt,hard_push,seg_fwd,stop_fwd,plate_fwd,hull_fwd,solver_up,sense_speed,sense_r\n",
    );
    let gear = |bed: &Bed| {
        bed.app
            .world()
            .get::<crate::track::sim::TankTransmission>(bed.tank)
            .expect("a tank carries its transmission")
            .0
            .gear
    };
    let mut shifts = Vec::new();
    for tick in 0..DRIVE_S * TICKS {
        let from = gear(&bed);
        bed.tick(throttle, steer);
        let to = gear(&bed);
        if to != from {
            shifts.push(json!([tick, from, to]));
        }
        csv += &format!("{tick},{}\n", row(&mut bed, begin, forward));
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/creep-stall");
    std::fs::create_dir_all(&dir).expect("the target directory exists or can be made");
    std::fs::write(dir.join(format!("{tag}-{case}-s{seed}.csv")), csv)
        .expect("the per-tick series writes");
    let end = bed.hull();
    let line = json!({
        "tag": tag,
        "case": case,
        "seed": seed,
        "throttle": throttle,
        "weight_n": bed.weight(),
        "begin": [begin.x, begin.y, begin.z],
        "heap_near_far_z": heap,
        "travel_m": (end.position - begin).dot(forward),
        "rise_m": end.position.y - begin.y,
        "end_speed_m_s": end.speed(),
        "shifts": shifts,
    });
    println!("CREEPSTALL {line}");
}
