//! Class invariants of the witness pass (ADR-0040) on generic fixtures: fixture plates and boxes,
//! never a tank's own volumes. Each invariant is checked against its mutant, the same volume as
//! a soft hard box (a layer of the walk), which it must tell apart.

use bevy::prelude::*;

use crate::ballistics::march_tests::{FIXTURE_88, FIXTURE_MG};
use crate::ballistics::pen_cam::{
    FixtureFired, FixtureVolume, Kind, LINE_FROM, THICK, THIN, fixture_fire, fixture_fire_with,
    plate,
};
use crate::ballistics::{ArmourMark, CrossingEnd, RoundBody, VolumeCrossing};
use crate::damage_cam::{BoxDose, RoundEnd, ShotVerdict};

/// Fire the fixture 88 down the head-on line at `volumes`, all one tank's, with its wound rate set
/// to `wound`.
fn fire(volumes: &[FixtureVolume], wound: f32) -> FixtureFired {
    let owned: Vec<(FixtureVolume, usize, Quat)> = volumes
        .iter()
        .map(|&volume| (volume, 0, Quat::IDENTITY))
        .collect();
    fixture_fire(&owned, LINE_FROM, Vec3::NEG_Z, 0, |projectile| {
        projectile.wound = wound;
    })
}

/// The fixture wound rate (reference-mm per metre), the main gun's tuned value.
const WOUND: f32 = 30.0;

/// HP pool no fixture dose empties, so no death changes what is read.
const DEEP: u32 = 10_000;

/// A cube of side `side` (m) centred at `at`.
fn cube(name: &'static str, side: f32, at: Vec3, kind: Kind) -> FixtureVolume {
    (name, Vec3::splat(side), at, kind)
}

/// Everything of the shot but the volumes named `witnesses`: the shell's path, end state, plates
/// and bounces, every armour mark but the witness pass's, every spall direction's strength and
/// reach, and every other box's dose and state. Debug renders every float to its shortest
/// round-trip digits, so equal renderings are equal bits.
fn hard_world(fired: &FixtureFired, witnesses: &[&str]) -> String {
    let witness = |name: &str| witnesses.contains(&name);
    let mut report = fired.report.clone();
    report.doses.retain(|dose| !witness(&dose.volume));
    report.components.retain(|state| !witness(&state.name));
    report.consequences.retain(|c| !witness(&c.volume));
    for cone in &mut report.spall {
        for fragment in &mut cone.fragments {
            fragment.hits.retain(|hit| !witness(&hit.volume));
        }
    }
    let log: Vec<&ArmourMark> = fired
        .log
        .iter()
        .filter(|mark| !matches!(mark, ArmourMark::Wound { .. }))
        .collect();
    format!(
        "{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}",
        report.impact,
        report.plates,
        report.ricochets,
        report.path,
        report.spall,
        report.end,
        report.doses,
        report.components,
        report.consequences,
        report.killed,
        log,
        fired.at,
        fired.velocity,
    )
}

/// The record's dose to `name`, if any.
fn dose<'a>(fired: &'a FixtureFired, name: &str) -> Option<&'a BoxDose> {
    fired.report.doses.iter().find(|dose| dose.volume == name)
}

/// Every spall direction's dose to `name`, in cone and direction order.
fn fragment_doses(fired: &FixtureFired, name: &str) -> Vec<f32> {
    fired
        .report
        .spall
        .iter()
        .flat_map(|cone| &cone.fragments)
        .flat_map(|fragment| &fragment.hits)
        .filter(|hit| hit.volume == name)
        .map(|hit| hit.damage)
        .collect()
}

/// The witness pass's crossings of `name`, one per [`ArmourMark::Wound`] that met it.
fn wounds<'a>(fired: &'a FixtureFired, name: &str) -> Vec<&'a VolumeCrossing> {
    fired
        .log
        .iter()
        .filter_map(|mark| match mark {
            ArmourMark::Wound { volumes, .. } => Some(volumes),
            _ => None,
        })
        .flatten()
        .filter(|crossed| fired.names.get(&crossed.volume).map(String::as_str) == Some(name))
        .collect()
}

/// A perforated front plate, a hard module off the line in the cone's reach, and a stopping rear
/// plate (fixture data): the hard world every witness test fires into, `extra` appended last so
/// every hard volume keeps its entity.
fn hard_fixture(extra: &[FixtureVolume]) -> Vec<FixtureVolume> {
    let mut volumes = vec![
        plate("front", THIN, 0.0, Kind::Main),
        cube("module", 0.4, Vec3::new(0.3, 2.0, -2.2), Kind::Box(DEEP)),
        plate("rear", THICK, -3.0, Kind::Main),
    ];
    volumes.extend_from_slice(extra);
    volumes
}

/// The witness on the line and in the cone, ahead of the module.
fn crewman(kind: Kind) -> FixtureVolume {
    cube("crewman", 0.6, Vec3::new(0.0, 2.0, -1.2), kind)
}

/// A witness never changes the hard world: the shell's path and end state, every armour mark,
/// every spall direction's reach, and every hard box's dose read the same with it as without it,
/// while it is dosed by both the line and the field.
#[test]
fn a_witness_never_changes_the_hard_world() {
    let alone = fire(&hard_fixture(&[]), WOUND);
    let with = fire(&hard_fixture(&[crewman(Kind::Witness(DEEP))]), WOUND);
    let hard = hard_world(&alone, &["crewman"]);
    assert_eq!(hard_world(&with, &["crewman"]), hard);
    let crewman_dose = dose(&with, "crewman").expect("the witness is dosed");
    assert!(
        crewman_dose.line > 0.0 && crewman_dose.field > 0.0,
        "{crewman_dose:?}"
    );
    assert!(
        dose(&with, "module").is_some_and(|dose| dose.field > 0.0),
        "the module is in the cone's reach: the fixture tests shadowing"
    );
    // MUTANT: the same volume as a soft hard box bends, slows or shadows; the comparison sees it.
    let layer = fire(&hard_fixture(&[crewman(Kind::Box(DEEP))]), WOUND);
    assert_ne!(
        hard_world(&layer, &["crewman"]),
        hard,
        "a soft hard box in the crewman's place must change the hard world"
    );
}

/// A witness behind a witness takes the dose it would take alone: line, field, HP lost, entry and
/// every spall direction's share, bit for bit.
#[test]
fn a_witness_behind_a_witness_is_dosed_as_alone() {
    let rear = cube(
        "rear crewman",
        0.5,
        Vec3::new(0.05, 2.0, -1.9),
        Kind::Witness(DEEP),
    );
    let front = |kind| cube("front crewman", 0.5, Vec3::new(0.0, 2.0, -1.0), kind);
    let alone = fire(&hard_fixture(&[rear]), WOUND);
    let behind = fire(&hard_fixture(&[front(Kind::Witness(DEEP)), rear]), WOUND);
    let read = |fired: &FixtureFired| {
        (
            dose(fired, "rear crewman").cloned(),
            fragment_doses(fired, "rear crewman"),
        )
    };
    let expected = read(&alone);
    let rear_dose = expected.0.as_ref().expect("the rear witness is dosed");
    assert!(
        rear_dose.line > 0.0 && rear_dose.field > 0.0,
        "{rear_dose:?}"
    );
    assert!(
        dose(&behind, "front crewman").is_some_and(|dose| dose.line > 0.0),
        "the front witness stands on the line"
    );
    assert_eq!(read(&behind), expected);
    // MUTANT: a soft hard box in front shadows the one behind.
    let shadowed = fire(&hard_fixture(&[front(Kind::Box(DEEP)), rear]), WOUND);
    assert_ne!(
        read(&shadowed),
        expected,
        "a soft hard box in front must change the rear witness's dose"
    );
}

/// The line dose is the disc-mean chord times the wound rate: a witness twice as deep along the
/// line takes twice the chord, and a graze (the axis outside, part of the disc inside) takes a
/// chord strictly between none and the full pass.
#[test]
fn the_line_dose_is_the_chord_times_the_wound_rate() {
    let face = 0.5;
    let pass = |depth: f32, x: f32| {
        let witness = (
            "crewman",
            Vec3::new(face, face, depth),
            Vec3::new(x, 2.0, -1.2),
            Kind::Witness(DEEP),
        );
        let fired = fire(&hard_fixture(&[witness]), WOUND);
        let crossed = wounds(&fired, "crewman");
        assert_eq!(crossed.len(), 1, "one leg meets it");
        let crossed = crossed[0];
        assert_eq!(crossed.cost, crossed.chord * WOUND, "dose = chord × wound");
        let line = dose(&fired, "crewman").map_or(0.0, |dose| dose.line);
        assert_eq!(
            line,
            crate::ballistics::hp_before_rounding(crate::ballistics::dose_units(crossed.cost)),
            "the record's line is the pass's dose, in HP points at the reference-mm rate"
        );
        crossed.chord
    };
    let (shallow, deep) = (pass(0.3, 0.0), pass(0.6, 0.0));
    // Head-on, every disc sample crosses the whole depth: f32 rounding only (fixture tolerance).
    assert!((shallow - 0.3).abs() < 1.0e-4, "{shallow}");
    assert!(
        (deep - 2.0 * shallow).abs() < 1.0e-4,
        "{deep} vs 2 × {shallow}"
    );
    // The near x face 20 mm off the axis, inside the 44 mm disc radius (fixture geometry).
    let graze = pass(0.6, 0.02 + face / 2.0);
    assert!(0.0 < graze && graze < deep, "{graze} vs {deep}");
}

/// The wound rate is round data alone: two rounds that differ only in it cross the same chords,
/// leave the hard world and the field doses the same, and dose the line in proportion.
#[test]
fn the_wound_rate_is_round_data_only() {
    let volumes = hard_fixture(&[crewman(Kind::Witness(DEEP))]);
    let (heavy, light) = (30.0, 10.0);
    let a = fire(&volumes, heavy);
    let b = fire(&volumes, light);
    assert_eq!(hard_world(&a, &["crewman"]), hard_world(&b, &["crewman"]));
    let (wa, wb) = (wounds(&a, "crewman"), wounds(&b, "crewman"));
    assert!(!wa.is_empty());
    assert_eq!(wa.len(), wb.len());
    for (x, y) in wa.iter().zip(&wb) {
        assert_eq!(x.chord, y.chord, "the chord is geometry, not round data");
        assert_eq!(x.cost, x.chord * heavy);
        assert_eq!(y.cost, y.chord * light);
    }
    assert_eq!(
        fragment_doses(&a, "crewman"),
        fragment_doses(&b, "crewman"),
        "the field dose is the hard world's, not the wound rate's"
    );
    let (da, db) = (dose(&a, "crewman").unwrap(), dose(&b, "crewman").unwrap());
    assert!(da.line > db.line, "{da:?} vs {db:?}");
}

/// Fire a round of `body` at `speed` (m/s) down the head-on line at `volumes`, each turned by its
/// own rotation and all one tank's, with its wound rate set to `wound`, into the fixture world as
/// `setup` leaves it.
fn fire_turned(
    volumes: &[(FixtureVolume, Quat)],
    body: RoundBody,
    speed: f32,
    wound: f32,
    setup: impl FnOnce(&mut App),
) -> FixtureFired {
    let owned: Vec<(FixtureVolume, usize, Quat)> = volumes
        .iter()
        .map(|&(volume, turn)| (volume, 0, turn))
        .collect();
    fixture_fire_with(
        &owned,
        LINE_FROM,
        Vec3::NEG_Z * speed,
        0,
        body,
        |projectile| projectile.wound = wound,
        setup,
    )
}

/// [`fire_turned`] with every volume square to the line.
fn fire_square(
    volumes: &[FixtureVolume],
    body: RoundBody,
    speed: f32,
    wound: f32,
    setup: impl FnOnce(&mut App),
) -> FixtureFired {
    let square: Vec<(FixtureVolume, Quat)> = volumes
        .iter()
        .map(|&volume| (volume, Quat::IDENTITY))
        .collect();
    fire_turned(&square, body, speed, wound, setup)
}

/// The fixture 88's muzzle speed (m/s), as [`fire`] fires it: fixture data.
const SPEED_88: f32 = 800.0;

/// The fixture MG round's speed (m/s) and wound rate (reference-mm per metre): fixture data.
const SPEED_MG: f32 = 750.0;
const WOUND_MG: f32 = 10.0;

/// Where the march logged the witness pass's crossings of `name` relative to its hard marks: the
/// index of each [`ArmourMark::Wound`] that met it, and the index of the last hard mark.
fn wound_marks(fired: &FixtureFired, name: &str) -> (Vec<usize>, Option<usize>) {
    let met = fired
        .log
        .iter()
        .enumerate()
        .filter_map(|(index, mark)| match mark {
            ArmourMark::Wound { volumes, .. } => volumes
                .iter()
                .any(|crossed| fired.names.get(&crossed.volume).map(String::as_str) == Some(name))
                .then_some(index),
            _ => None,
        })
        .collect();
    let hard = fired
        .log
        .iter()
        .rposition(|mark| !matches!(mark, ArmourMark::Wound { .. }));
    (met, hard)
}

/// A witness the round meets in open air, past every hard body, is dosed and settled by the leg
/// that flies through it: the line dose is recorded and the HP is taken.
#[test]
fn a_leg_into_open_air_doses_and_settles_its_witness() {
    let volumes = [
        plate("front", THIN, 0.0, Kind::Main),
        crewman(Kind::Witness(DEEP)),
    ];
    let fired = fire(&volumes, WOUND);
    let (met, hard) = wound_marks(&fired, "crewman");
    let hard = hard.expect("the plate is crossed");
    assert!(
        met.iter().any(|&index| index > hard),
        "a leg past the hard world met the witness: {met:?} after {hard}"
    );
    let crewman_dose = dose(&fired, "crewman").expect("the witness is dosed");
    assert!(
        crewman_dose.line > 0.0 && crewman_dose.taken > 0,
        "{crewman_dose:?}"
    );
}

/// An automatic round with a wound rate wounds a witness in the open, no armour on the line: the
/// witness is no face to strike or bounce off, so the march logs no hard mark at all, only the
/// line's dose at the round's own rate.
#[test]
fn an_automatic_round_wounds_a_witness_in_the_open() {
    let fired = fire_square(
        &[crewman(Kind::Witness(DEEP))],
        FIXTURE_MG,
        SPEED_MG,
        WOUND_MG,
        |_| {},
    );
    assert!(
        fired
            .log
            .iter()
            .all(|mark| matches!(mark, ArmourMark::Wound { .. })),
        "a witness is never struck or crossed as a hard body: {:?}",
        fired.log
    );
    let crossed = wounds(&fired, "crewman");
    assert!(!crossed.is_empty(), "the line met the witness");
    let chord: f32 = crossed.iter().map(|crossed| crossed.chord).sum();
    // Head-on through a 0.6 m cube (fixture geometry): f32 rounding only.
    assert!((chord - 0.6).abs() < 1.0e-4, "{chord}");
    for crossing in &crossed {
        assert_eq!(
            crossing.cost,
            crossing.chord * WOUND_MG,
            "the round's own rate"
        );
    }
    let crewman_dose = dose(&fired, "crewman").expect("the witness is dosed");
    assert!(
        crewman_dose.line > 0.0 && crewman_dose.taken > 0,
        "{crewman_dose:?}"
    );
}

/// A witness the round meets with no hard body anywhere on its flight still has an explanation:
/// the record opens on the witness's damage, with the line's deposit, where the line entered it,
/// the path timed from that entry, and the round's end where it left.
#[test]
fn a_witness_struck_alone_has_a_report() {
    let fired = fire_square(
        &[crewman(Kind::Witness(DEEP))],
        FIXTURE_88,
        SPEED_88,
        WOUND,
        |_| {},
    );
    let report = &fired.report;
    assert_eq!(report.impact.verdict, ShotVerdict::Damaged);
    let crewman_dose = dose(&fired, "crewman").expect("the witness's deposit is recorded");
    assert!(
        crewman_dose.line > 0.0 && crewman_dose.taken > 0,
        "{crewman_dose:?}"
    );
    // The crewman's faces along the line: z = -0.9 in, z = -1.5 out (fixture geometry).
    let entered = crewman_dose.entered.expect("the line's entry is recorded");
    assert!((entered[2] + 0.9).abs() < 1.0e-3, "{entered:?}");
    let contact = report
        .path
        .iter()
        .find(|node| node.t == 0.0)
        .expect("the path is timed from first contact");
    assert!((contact.point[2] + 0.9).abs() < 1.0e-3, "{contact:?}");
    assert!(
        report.path.windows(2).all(|pair| pair[0].t <= pair[1].t),
        "{:?}",
        report.path
    );
    match &report.end {
        RoundEnd::Left { at } => assert!((at[2] + 1.5).abs() < 1.0e-3, "{at:?}"),
        end => panic!("the round flies on past the witness: {end:?}"),
    }
}

/// The wall case: a cone born inside a witness (a crewman pressed against the plate's inner face)
/// doses it from the cone's origin, every direction its full thrown share.
#[test]
fn a_cone_born_inside_a_witness_doses_it_from_its_origin() {
    // The crewman overlaps the plate by 0.1 m, so the plate's exit lies inside it (fixture
    // geometry).
    let pressed = cube(
        "crewman",
        0.6,
        Vec3::new(0.0, 2.0, -THIN / 2.0 - 0.3 + 0.1),
        Kind::Witness(DEEP),
    );
    let fired = fire(
        &[
            plate("front", THIN, 0.0, Kind::Main),
            pressed,
            plate("rear", THICK, -3.0, Kind::Main),
        ],
        WOUND,
    );
    assert_eq!(
        fired.report.spall.len(),
        1,
        "the front plate's exit throws one cone"
    );
    let cone = &fired.report.spall[0];
    let weight = 1.0 / cone.fragments.len() as f32;
    for fragment in &cone.fragments {
        let hit = fragment
            .hits
            .iter()
            .find(|hit| hit.volume == "crewman")
            .expect("every direction enters the witness at its origin");
        assert_eq!(
            hit.damage,
            (fragment.strength as f64 * weight as f64) as f32,
            "the direction's whole share"
        );
    }
}

/// A witness is dosed at the strength that reaches it: each direction doses it exactly what it
/// doses a hard box in its place, and a direction a hard box stops short of it doses it nothing,
/// every other direction what it would with that box gone. A witness dosed by other arithmetic
/// than a hard box's, or dosed past the hard body that stopped the direction, fails it.
#[test]
fn a_witness_takes_the_strength_that_reaches_it() {
    let fire_as = |kind: Kind, module: bool| {
        let mut volumes = vec![plate("front", THIN, 0.0, Kind::Main)];
        if module {
            // Off the shell's line to one side, between the front plate's exit and the crewman, so
            // the cone meets the crewman both past it and, with it gone, through where it stood
            // (fixture geometry).
            volumes.push(cube(
                "module",
                0.4,
                Vec3::new(0.25, 2.0, -0.6),
                Kind::Box(DEEP),
            ));
        }
        volumes.extend([crewman(kind), plate("rear", THICK, -3.0, Kind::Main)]);
        fire_square(&volumes, FIXTURE_88, SPEED_88, WOUND, |_| {})
    };
    let cone_doses = |fired: &FixtureFired| -> Vec<(Option<f32>, Option<f32>)> {
        fired.report.spall[0]
            .fragments
            .iter()
            .map(|fragment| {
                let find = |name: &str| {
                    fragment
                        .hits
                        .iter()
                        .find(|hit| hit.volume == name)
                        .map(|hit| hit.damage)
                };
                (find("crewman"), find("module"))
            })
            .collect()
    };
    let witnessed = cone_doses(&fire_as(Kind::Witness(DEEP), true));
    assert_eq!(
        witnessed,
        cone_doses(&fire_as(Kind::Box(DEEP), true)),
        "(crewman, module) per direction"
    );
    let alone = cone_doses(&fire_as(Kind::Witness(DEEP), false));
    let (mut shadowed, mut clear) = (0, 0);
    for (index, ((crewman, module), (unshadowed, _))) in witnessed.iter().zip(&alone).enumerate() {
        let expected = if module.is_some() { None } else { *unshadowed };
        assert_eq!(*crewman, expected, "direction {index}");
        if module.is_some() && unshadowed.is_some() {
            shadowed += 1;
        } else if crewman.is_some() {
            clear += 1;
        }
    }
    assert!(
        shadowed > 0 && clear > 0,
        "the fixture's cone meets the crewman both behind the module and past it: {shadowed}, \
         {clear}"
    );
}

/// A witness whose own pass fails (one too large for any corridor, fixture geometry) leaves the
/// hard world whole: every hard dose, every spall direction's reach and the shell's path as with
/// no witness at all, and the witness dosed by nothing.
#[test]
fn a_failed_witness_pass_keeps_the_hard_world() {
    let giant = cube(
        "giant",
        60.0,
        Vec3::new(0.0, 2.0, -1.5),
        Kind::Witness(DEEP),
    );
    let alone = fire(&hard_fixture(&[]), WOUND);
    let with = fire(&hard_fixture(&[giant]), WOUND);
    assert_eq!(
        hard_world(&with, &["giant"]),
        hard_world(&alone, &["giant"])
    );
    assert!(dose(&with, "giant").is_none(), "{:?}", dose(&with, "giant"));
    assert!(
        dose(&with, "module").is_some_and(|dose| dose.field > 0.0),
        "the hard world's own doses stand"
    );
}

/// A spall direction with no hard body ahead ends at the struck tank's horizon, never at its throw
/// point, and its reach is the same with a witness on it as without.
#[test]
fn a_direction_with_no_hard_body_ahead_reaches_the_horizon() {
    // A side wall off the line: some directions end on it, the rest meet nothing (fixture
    // geometry).
    let wall = (
        "wall",
        Vec3::new(0.05, 3.0, 3.0),
        Vec3::new(2.5, 2.0, -1.5),
        Kind::Main,
    );
    let front = plate("front", THIN, 0.0, Kind::Main);
    let alone = fire(&[front, wall], WOUND);
    let with = fire(&[front, wall, crewman(Kind::Witness(DEEP))], WOUND);
    // The spall and the hard log; the record's path and end go on through a witness the shell
    // meets after its last hard body.
    let spall = |fired: &FixtureFired| {
        let mut spall = fired.report.spall.clone();
        for fragment in spall.iter_mut().flat_map(|cone| &mut cone.fragments) {
            fragment.hits.retain(|hit| hit.volume != "crewman");
        }
        let log: Vec<&ArmourMark> = fired
            .log
            .iter()
            .filter(|mark| !matches!(mark, ArmourMark::Wound { .. }))
            .collect();
        format!("{spall:?}\n{log:?}")
    };
    assert_eq!(spall(&with), spall(&alone));
    assert!(
        dose(&with, "crewman").is_some_and(|dose| dose.field > 0.0),
        "the field reaches the witness"
    );
    let cone = &alone.report.spall[0];
    let origin = Vec3::from(cone.origin);
    // The union of the plate's and the wall's bounds lies at least 1.5 m ahead of the plate's exit
    // in every forward direction (DERIVED from the fixture geometry).
    for fragment in &cone.fragments {
        let reach = Vec3::from(fragment.end).distance(origin);
        assert!(reach > 1.0, "{reach} from {origin}");
    }
}

/// At an oblique bend, each disc sample's line runs to its own handoff and on from there: a thin
/// witness lying flush in front of a sloped plate is crossed whole by every sample on the way in
/// and never again on the way through, so its disc-mean chord is its depth over the cosine of the
/// slope (DERIVED), whatever the samples' spread along the line.
#[test]
fn a_witness_beside_an_oblique_bend_is_read_along_each_samples_own_path() {
    let slope = 40.0_f32.to_radians();
    let turn = Quat::from_rotation_x(slope);
    let normal = turn * Vec3::Z;
    let (plate_depth, depth, gap) = (0.02, 0.02, 0.001);
    let centre = Vec3::new(0.0, 2.0, 0.0);
    let sloped = (
        "front",
        Vec3::new(3.0, 3.0, plate_depth),
        centre,
        Kind::Main,
    );
    let film = (
        "film",
        Vec3::new(3.0, 3.0, depth),
        centre + normal * (plate_depth / 2.0 + gap + depth / 2.0),
        Kind::Witness(DEEP),
    );
    let fired = fire_turned(
        &[(sloped, turn), (film, turn)],
        FIXTURE_88,
        SPEED_88,
        WOUND,
        |_| {},
    );
    // The crossing's axis leaves its entrance off the incoming line by more than 0.8° (fixture
    // threshold).
    let bent = fired.log.iter().any(|mark| match mark {
        ArmourMark::Crossing {
            entrance,
            incoming,
            end: CrossingEnd::Exit { at, .. },
            ..
        } => (*at - *entrance).normalize().dot(*incoming) < 0.9999,
        _ => false,
    });
    assert!(bent, "the plate bends the round: the fixture tests a bend");
    let chord: f32 = wounds(&fired, "film")
        .iter()
        .map(|crossed| crossed.chord)
        .sum();
    let expected = depth / slope.cos();
    assert!(
        (chord - expected).abs() < 1.0e-4,
        "{chord} vs {expected} (DERIVED)"
    );
}

/// THE STRONGEST-ONLY RULE on the line (§13.2's damage law): a witness's chord inside a hard
/// volume takes no dose. A crew box clipped into a plate reads, on the line, as its open part
/// alone, and one wholly inside the plate is not met at all, as a field direction is stopped by
/// the plate's face before it. The HP tally and pen cam read these chords; a line that dosed the
/// overlapped chord would dose a box clipped into armour through the armour. Fixture geometry:
/// the front plate's back face at `z = -THIN / 2`, a 0.5 m square box over `z ∈ [-0.25, 0]`,
/// 0.225 m of it in the open (DERIVED); the buried box over `z ∈ [-0.015, 0.015]`.
#[test]
fn a_crew_box_clipped_into_a_wall_is_dosed_only_over_its_open_chord() {
    let box_over = |near: f32, far: f32| {
        (
            "crewman",
            Vec3::new(0.5, 0.5, near - far),
            Vec3::new(0.0, 2.0, (near + far) / 2.0),
            Kind::Witness(DEEP),
        )
    };
    let chord = |fired: &FixtureFired| -> f32 {
        wounds(fired, "crewman")
            .iter()
            .map(|crossed| crossed.chord)
            .sum()
    };
    let back = -THIN / 2.0;
    let clipped = fire(&hard_fixture(&[box_over(0.0, -0.25)]), WOUND);
    let open = fire(&hard_fixture(&[box_over(back, -0.25)]), WOUND);
    let (clipped_chord, open_chord) = (chord(&clipped), chord(&open));
    assert!(
        (clipped_chord - (back + 0.25)).abs() < 1.0e-4,
        "the clipped box's chord is its open part's: {clipped_chord} (DERIVED {})",
        back + 0.25
    );
    assert!(
        (clipped_chord - open_chord).abs() < 1.0e-4,
        "{clipped_chord} against the open part alone, {open_chord}"
    );
    for crossed in wounds(&clipped, "crewman") {
        assert_eq!(
            crossed.cost,
            crossed.chord * WOUND,
            "dose = open chord × wound"
        );
    }
    assert!(dose(&clipped, "crewman").is_some_and(|dose| dose.line > 0.0));

    let buried = fire(&hard_fixture(&[box_over(0.015, -0.015)]), WOUND);
    assert!(
        wounds(&buried, "crewman").is_empty(),
        "a box wholly inside the plate is not met by the line"
    );
    assert!(dose(&buried, "crewman").is_none_or(|dose| dose.line == 0.0));
}
