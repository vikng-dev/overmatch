//! Class invariants of the witness pass (ADR-0040) on generic fixtures: fixture plates and boxes,
//! never a tank's own volumes. Each invariant is checked against its mutant, the same volume as
//! a soft hard box (a layer of the walk), which it must tell apart.

use bevy::prelude::*;

use crate::ballistics::pen_cam::{
    FixtureFired, FixtureVolume, Kind, LINE_FROM, THICK, THIN, fixture_fire, plate,
};
use crate::ballistics::{ArmourMark, VolumeCrossing};
use crate::damage_cam::BoxDose;

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
        assert_eq!(line, crossed.cost, "the record's line is the pass's dose");
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
