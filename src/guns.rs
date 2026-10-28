//! The gun registry: every gun model, one file each (`assets/guns/<key>.gun.ron`, the file stem its
//! key), tank-independent. A gun's ammunition table lists every round it can chamber (a key of
//! [`crate::rounds`]) with the muzzle speed it gives that round; the [`Round`] the laws read is a
//! round fired from a gun, and nowhere else does a round get a speed.
//!
//! Entry points: [`GunRegistry::shipped`] (embedded, parsed once), [`GunRegistry::round`] (a round
//! through the gun that fires it), [`GunRegistry::gun`] (a gun's mechanism and report) and
//! [`GunRegistry::round_by_key`] (a round through the one gun whose table lists it, for a reader
//! that holds a round key and no gun).
//!
//! # A gun file
//!
//! `names`: the gun's display designations, several where one tube was named for several cradles;
//! `caliber`: its nominal calibre (m); `ammunition`: `{ "<round key>": (muzzle_speed: (m/s, Kind)) }`;
//! `mechanism`: `Single` (one round per trigger edge) or `Automatic(cyclic_rpm: (rpm, Kind))` (a
//! belt fired on a held trigger, one round every 60/rpm s); `report_clips`: the gun's report takes
//! (asset-relative paths, one rolled per round fired; `[]` is a silent gun, a decision). The names
//! and the calibre are validated and read by nothing yet. Every number is authored
//! `(value, Kind("source or reason"))` ([`crate::provenance`]); a value without one does not load.
//! A tank's mount names its gun ([`crate::spec::WeaponSpec::gun`]): the mount's reload or belt joins
//! the gun's mechanism there.
//!
//! # What loads
//!
//! Each a load error naming the gun: no name, or one the shipped font does not draw whole; a calibre
//! not finite and positive; a cyclic rate not finite and positive; an empty ammunition table, or one
//! naming a round twice; a row naming no round; a row whose round the force law cannot state at its
//! speed ([`RoundFacts::fired_at`]). And for the registry as a whole: a round no gun's table lists,
//! which no shot could ever fire.
//!
//! [`RoundFacts::fired_at`]: crate::rounds::RoundFacts::fired_at

use serde::Deserialize;
use std::collections::BTreeMap;

use crate::provenance::Sourced;
use crate::rounds::{KeyedMap, Round, RoundError, RoundRegistry};
use crate::spec::FireMechanism;

/// One gun as authored.
#[derive(Deserialize)]
#[serde(rename = "Gun", deny_unknown_fields)]
struct GunFile {
    names: Vec<Sourced<String>>,
    caliber: Sourced<f32>,
    ammunition: KeyedMap<AmmunitionFile>,
    mechanism: MechanismFile,
    report_clips: Vec<String>,
}

/// A gun's mechanism as authored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum MechanismFile {
    Single,
    Automatic {
        /// Rounds per minute.
        cyclic_rpm: Sourced<f32>,
    },
}

/// A gun's mechanism, validated. A mount's fire mode is of the same family
/// ([`crate::spec::WeaponSpec::fire_cycle`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Mechanism {
    /// One round per trigger edge; the reload is the mount's.
    Single,
    /// A belt fired on a held trigger, one round every `60 / cyclic_rpm` s — pure mechanism, never
    /// gated on crew; the belt is the mount's.
    Automatic { cyclic_rpm: f32 },
}

impl Mechanism {
    /// The family a mount's fire mode must share.
    pub(crate) fn family(self) -> FireMechanism {
        match self {
            Self::Single => FireMechanism::Single,
            Self::Automatic { .. } => FireMechanism::Automatic,
        }
    }
}

/// One row of a gun's ammunition table as authored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AmmunitionFile {
    /// m/s.
    muzzle_speed: Sourced<f32>,
}

/// One gun, validated: each round its table lists, fired at the speed the table gives it, its
/// mechanism and its report.
#[derive(Clone, Debug)]
pub(crate) struct Gun {
    ammunition: BTreeMap<String, Round>,
    mechanism: Mechanism,
    report_clips: Vec<String>,
}

impl Gun {
    pub(crate) fn mechanism(&self) -> Mechanism {
        self.mechanism
    }

    /// The report takes, asset-relative; empty for a silent gun.
    pub(crate) fn report_clips(&self) -> &[String] {
        &self.report_clips
    }
}

/// Why a gun registry, or a lookup in one, failed. Every arm names the offending text.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GunError {
    /// A gun file did not parse: names the gun.
    Parse { gun: String, err: String },
    /// A gun parsed but cannot fire: names the gun and the reason.
    Invalid { gun: String, reason: String },
    /// A lookup named a gun the registry does not carry; carries the registry's vocabulary.
    Unknown { name: String, known: Vec<String> },
    /// A lookup named a round the gun's ammunition table does not list; carries the table's keys.
    NotChambered {
        gun: String,
        round: String,
        chambered: Vec<String>,
    },
    /// A round no gun's ammunition table lists.
    Unchambered { round: String },
    /// A lookup by round alone named a round several guns' tables list.
    Ambiguous { round: String, guns: Vec<String> },
    /// The round registry's own error.
    Round(RoundError),
}

impl std::fmt::Display for GunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse { gun, err } => {
                write!(f, "assets/guns/{gun}.gun.ron failed to parse: {err}")
            }
            Self::Invalid { gun, reason } => write!(f, "gun {gun:?}: {reason}"),
            Self::Unknown { name, known } => write!(
                f,
                "unknown gun {name:?} — assets/guns/ holds [{}]",
                known.join(", ")
            ),
            Self::NotChambered {
                gun,
                round,
                chambered,
            } => write!(
                f,
                "gun {gun:?} does not chamber round {round:?} — its ammunition table lists [{}]",
                chambered.join(", ")
            ),
            Self::Unchambered { round } => write!(
                f,
                "round {round:?} is listed by no gun's ammunition table — no gun gives it a muzzle \
                 speed, so no shot can fire it"
            ),
            Self::Ambiguous { round, guns } => write!(
                f,
                "round {round:?} is chambered by several guns [{}] — name the gun that fires it",
                guns.join(", ")
            ),
            Self::Round(err) => write!(f, "{err}"),
        }
    }
}

/// Resolve one authored gun against `rounds`, or say why it cannot fire or be shown.
fn resolved(file: GunFile, rounds: &RoundRegistry) -> Result<Gun, String> {
    if file.names.is_empty() {
        return Err("names is empty — a gun is shown by at least one designation".into());
    }
    for Sourced(name) in &file.names {
        if crate::ui_font::drawable(name).as_deref() != Some(name.as_str()) {
            return Err(format!(
                "name {name:?} is not drawn whole by the shipped font"
            ));
        }
    }
    let caliber = file.caliber.0;
    if !caliber.is_finite() || caliber <= 0.0 {
        return Err(format!(
            "caliber must be finite and positive (got {caliber})"
        ));
    }
    let mechanism = match file.mechanism {
        MechanismFile::Single => Mechanism::Single,
        MechanismFile::Automatic {
            cyclic_rpm: Sourced(cyclic_rpm),
        } => {
            if !cyclic_rpm.is_finite() || cyclic_rpm <= 0.0 {
                return Err(format!(
                    "Automatic.cyclic_rpm must be finite and positive (the cyclic interval is \
                     60/rpm; got {cyclic_rpm})"
                ));
            }
            Mechanism::Automatic { cyclic_rpm }
        }
    };
    if file.ammunition.0.is_empty() {
        return Err("the ammunition table is empty — a gun chambers at least one round".into());
    }
    let ammunition = file
        .ammunition
        .0
        .into_iter()
        .map(|(key, row)| {
            let facts = rounds
                .get(&key)
                .map_err(|err| format!("ammunition: {err}"))?;
            let speed = row.muzzle_speed.0;
            let round = facts
                .fired_at(speed)
                .map_err(|reason| format!("ammunition {key:?} at {speed} m/s: {reason}"))?;
            Ok((key, round))
        })
        .collect::<Result<_, String>>()?;
    Ok(Gun {
        ammunition,
        mechanism,
        report_clips: file.report_clips,
    })
}

/// The parsed, validated gun files.
#[derive(Clone, Debug)]
pub(crate) struct GunRegistry {
    guns: BTreeMap<String, Gun>,
    /// The keys of the round registry the guns were resolved against, so a lookup tells a round no
    /// registry carries from one a gun does not chamber.
    rounds: Vec<String>,
}

impl GunRegistry {
    /// Parse a registry from gun files `files` (`(key, RON text)`), resolving each ammunition table
    /// against `rounds`. A key given twice, and a round of `rounds` no table lists, are refused.
    pub(crate) fn from_files(
        files: &[(&str, &str)],
        rounds: &RoundRegistry,
    ) -> Result<Self, GunError> {
        let mut guns = BTreeMap::new();
        for &(key, text) in files {
            let invalid = |reason: String| GunError::Invalid {
                gun: key.to_owned(),
                reason,
            };
            if guns.contains_key(key) {
                return Err(invalid(
                    "declared twice — a file's stem is its gun's key, so a duplicate is two answers \
                     for one name"
                        .into(),
                ));
            }
            let file: GunFile = ron::de::from_str(text).map_err(|err| GunError::Parse {
                gun: key.to_owned(),
                err: err.to_string(),
            })?;
            guns.insert(key.to_owned(), resolved(file, rounds).map_err(invalid)?);
        }
        let registry = Self {
            guns,
            rounds: rounds.keys().map(str::to_owned).collect(),
        };
        if let Some(round) = registry.rounds.iter().find(|round| {
            !registry
                .guns
                .values()
                .any(|gun| gun.ammunition.contains_key(*round))
        }) {
            return Err(GunError::Unchambered {
                round: round.clone(),
            });
        }
        Ok(registry)
    }

    /// The shipped registry: every embedded gun file ([`crate::armaments_index::GUN_FILES`]),
    /// parsed once against the shipped round registry. Panics on a malformed file: a broken
    /// authored contract is a build defect, as for the round registry.
    pub(crate) fn shipped() -> &'static Self {
        static REGISTRY: std::sync::LazyLock<GunRegistry> = std::sync::LazyLock::new(|| {
            GunRegistry::from_files(crate::armaments_index::GUN_FILES, RoundRegistry::shipped())
                .unwrap_or_else(|err| panic!("guns: the embedded gun files are unusable: {err}"))
        });
        &REGISTRY
    }

    /// The gun keyed `gun`, or the error naming it.
    pub(crate) fn gun(&self, gun: &str) -> Result<&Gun, GunError> {
        self.guns.get(gun).ok_or_else(|| GunError::Unknown {
            name: gun.to_owned(),
            known: self.guns.keys().cloned().collect(),
        })
    }

    /// Round `round` fired from gun `gun`, at the speed its ammunition table gives it; a hard error
    /// naming an unknown gun, an unknown round, or a round the gun does not chamber.
    pub(crate) fn round(&self, gun: &str, round: &str) -> Result<Round, GunError> {
        let table = &self.gun(gun)?.ammunition;
        table.get(round).copied().ok_or_else(|| {
            if self.rounds.iter().any(|key| key == round) {
                GunError::NotChambered {
                    gun: gun.to_owned(),
                    round: round.to_owned(),
                    chambered: table.keys().cloned().collect(),
                }
            } else {
                self.unknown_round(round)
            }
        })
    }

    /// Round `round` fired from the one gun whose ammunition table lists it; an error naming the
    /// round when no gun's table does or several do.
    pub(crate) fn round_by_key(&self, round: &str) -> Result<Round, GunError> {
        let guns: Vec<&str> = self
            .guns
            .iter()
            .filter(|(_, gun)| gun.ammunition.contains_key(round))
            .map(|(key, _)| key.as_str())
            .collect();
        match guns.as_slice() {
            [gun] => self.round(gun, round),
            [] if self.rounds.iter().any(|key| key == round) => Err(GunError::Unchambered {
                round: round.to_owned(),
            }),
            [] => Err(self.unknown_round(round)),
            several => Err(GunError::Ambiguous {
                round: round.to_owned(),
                guns: several.iter().map(|gun| (*gun).to_owned()).collect(),
            }),
        }
    }

    fn unknown_round(&self, round: &str) -> GunError {
        GunError::Round(RoundError::Unknown {
            name: round.to_owned(),
            known: self.rounds.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ballistics::RoundBody;
    use crate::ballistics::burst::Burst;
    use crate::rounds::tests::{FIXTURE, FIXTURE_TYPES, file};
    use crate::substances::{MaterialFacts, SubstanceRegistry};

    /// A round registry of the fixture round under each key of `keys`. Fixture data.
    fn fixture_rounds(keys: &[&str]) -> RoundRegistry {
        let text = file(&FIXTURE, "");
        let files: Vec<(&str, &str)> = keys.iter().map(|&key| (key, text.as_str())).collect();
        RoundRegistry::from_files(&files, FIXTURE_TYPES, SubstanceRegistry::shipped_static())
            .expect("the fixture rounds load")
    }

    /// A gun file of name `name`, calibre `caliber` and ammunition `rows` (`(round, speed)`), a
    /// single-shot silent gun, each value with a fixture provenance.
    fn gun_file(name: &str, caliber: &str, rows: &[(&str, &str)]) -> String {
        gun_file_with(name, caliber, rows, "Single")
    }

    /// [`gun_file`] with mechanism `mechanism` as authored.
    fn gun_file_with(name: &str, caliber: &str, rows: &[(&str, &str)], mechanism: &str) -> String {
        let rows = rows
            .iter()
            .map(|(round, speed)| {
                format!(r#""{round}": (muzzle_speed: ({speed}, Tuned("fixture")))"#)
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            r#"(names: [({name:?}, Tuned("fixture"))], caliber: ({caliber}, Tuned("fixture")),
                ammunition: {{ {rows} }}, mechanism: {mechanism}, report_clips: [])"#
        )
    }

    /// GunSteel's facts as the substance table states them: fixture data for [`PINS`].
    const GUN_STEEL: MaterialFacts = MaterialFacts {
        density: 7800.0,
        flow_strength: 1.18e9,
        youngs_modulus: 190.0e9,
        poisson_ratio: 0.29,
    };

    /// The Pzgr. 39 body every 88 mm full-calibre pin shares: fixture data for [`PINS`].
    const PZGR_39: RoundBody = RoundBody {
        mass: 10.2,
        caliber: 0.088,
        nose_factor: 0.291_666_66,
    };

    /// Fixture data: every shipped round's resolved record, to the bit — the values the laws, the
    /// shot and every pinned digest are measured against, read off the shipped data. A data edit
    /// that moves any of them moves the game's ballistics, and moves its pin here deliberately.
    const PINS: [(&str, Round); 4] = [
        (
            "7.9mm_AP",
            Round {
                body: RoundBody {
                    mass: 0.0118,
                    caliber: 0.0079,
                    nose_factor: 0.5,
                },
                flying: RoundBody {
                    mass: 0.0118,
                    caliber: 0.0079,
                    nose_factor: 0.5,
                },
                penetrator: RoundBody {
                    mass: 0.0118,
                    caliber: 0.0079,
                    nose_factor: 0.5,
                },
                muzzle_speed: 755.0,
                drag_k: 6.733_864e-4,
                wound: 10.0,
                core: GUN_STEEL,
                burst: None,
            },
        ),
        (
            "88mm_AP",
            Round {
                body: PZGR_39,
                flying: PZGR_39,
                penetrator: PZGR_39,
                muzzle_speed: 773.0,
                drag_k: 1.080_940_9e-4,
                wound: 30.0,
                core: GUN_STEEL,
                burst: None,
            },
        ),
        (
            "88mm_APCR",
            Round {
                body: RoundBody {
                    mass: 7.3,
                    caliber: 0.088,
                    nose_factor: 0.291_666_66,
                },
                flying: RoundBody {
                    mass: 7.3,
                    caliber: 0.088,
                    nose_factor: 0.291_666_66,
                },
                penetrator: RoundBody {
                    mass: 1.93,
                    caliber: 0.0357,
                    nose_factor: 0.291_666_66,
                },
                muzzle_speed: 930.0,
                drag_k: 2.789_961_6e-4,
                wound: 30.0,
                core: MaterialFacts {
                    density: 14900.0,
                    flow_strength: 4.585e9,
                    youngs_modulus: 627.0e9,
                    poisson_ratio: 0.21,
                },
                burst: None,
            },
        ),
        (
            "88mm_APHE_ring",
            Round {
                body: PZGR_39,
                flying: PZGR_39,
                penetrator: PZGR_39,
                muzzle_speed: 773.0,
                drag_k: 1.080_940_9e-4,
                wound: 30.0,
                core: GUN_STEEL,
                burst: Some(Burst {
                    arming_spend: 35.0,
                    delay: 0.0015,
                    nose: RoundBody {
                        mass: 8.370_217,
                        caliber: 0.088,
                        nose_factor: 0.291_666_66,
                    },
                    sideways_speed: 541.4819,
                    energy: 211_073.97,
                }),
            },
        ),
    ];

    /// THE PURE-REFACTOR GATE. Every shipped round, read through the gun that fires it, is field for
    /// field its pinned record ([`PINS`], to the bit); and so is every round the shipped tank
    /// stows, read through its mount's gun as spawn reads it. The laws, the shot and every pinned
    /// digest read this record, so a mutant that moves any row's muzzle speed, any type's or
    /// round's nose, a core, a crew line rate or a burst input fails here.
    #[test]
    fn every_round_reads_through_its_gun_bit_for_bit_as_pinned() {
        let guns = GunRegistry::shipped();
        let pinned: BTreeMap<&str, Round> = PINS.into_iter().collect();
        let shipped: Vec<&str> = RoundRegistry::shipped().keys().collect();
        assert_eq!(
            shipped,
            pinned.keys().copied().collect::<Vec<_>>(),
            "every shipped round is pinned"
        );
        for (key, pin) in &pinned {
            let round = guns
                .round_by_key(key)
                .unwrap_or_else(|err| panic!("{key}: {err}"));
            assert_eq!(&round, pin, "{key}");
        }
        let sheet: crate::spec::TankSpec =
            ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron"))
                .expect("the shipped sheet parses");
        for (mount, weapon) in &sheet.weapons {
            let rounds = weapon
                .resolved_rounds()
                .unwrap_or_else(|err| panic!("{mount}: {err}"));
            assert_eq!(rounds.len(), weapon.stowage.len(), "{mount}");
            for (stowed, round) in weapon.stowage.iter().zip(rounds) {
                let key = stowed.round.as_str();
                let pin = pinned
                    .get(key)
                    .unwrap_or_else(|| panic!("{mount}: {key} is not pinned"));
                assert_eq!(&round, pin, "{mount}: {key}");
                assert_eq!(
                    guns.round(&weapon.gun, key).as_ref(),
                    Ok(pin),
                    "{mount}: {key}"
                );
            }
        }
    }

    /// Every shipped gun loads, every shipped round is chambered by exactly one of them, and every
    /// row of every table is inside the force law's domain (`fired_at`, at load). A mutant that
    /// lets a round no gun lists load leaves it with no speed, a shot nothing can fire. Every
    /// report take names an `.ogg`: the sound loader resolves the path verbatim.
    #[test]
    fn every_shipped_round_is_chambered_by_one_shipped_gun() {
        let guns = GunRegistry::shipped();
        assert!(!guns.guns.is_empty());
        for key in RoundRegistry::shipped().keys() {
            guns.round_by_key(key)
                .unwrap_or_else(|err| panic!("{key}: {err}"));
        }
        for (key, gun) in &guns.guns {
            for clip in gun.report_clips() {
                assert!(
                    clip.ends_with(".ogg"),
                    "gun {key:?}: report clip `{clip}` must name an .ogg"
                );
            }
        }
    }

    /// A round the round registry carries but no gun's table lists fails the registry's load,
    /// naming the round: the load is where an unfirable round is caught, not the first shot.
    #[test]
    fn a_round_no_gun_chambers_fails_the_load() {
        let rounds = fixture_rounds(&["Probe", "Orphan"]);
        let err = GunRegistry::from_files(
            &[("Gun", &gun_file("Gun Mk. 1", "0.05", &[("Probe", "700.0")]))],
            &rounds,
        )
        .unwrap_err();
        assert_eq!(
            err,
            GunError::Unchambered {
                round: "Orphan".into()
            }
        );
    }

    /// Each refusal of one gun file names the gun and the field: no name or an undrawable one, a
    /// calibre not finite and positive, a cyclic rate not finite and positive, an empty table, a row
    /// naming no round, a round outside the force law's domain at its speed, a speed or a cyclic
    /// rate without provenance and a round listed twice. Every one would otherwise hand the shot a
    /// round it cannot state, a belt with no cyclic interval, or a gun nobody can show.
    #[test]
    fn a_gun_that_cannot_fire_or_be_shown_is_refused() {
        let rounds = fixture_rounds(&["Probe"]);
        let load = |text: String| GunRegistry::from_files(&[("Gun", &text)], &rounds);
        load(gun_file("Gun Mk. 1", "0.05", &[("Probe", "700.0")])).expect("the fixture gun loads");
        let belt =
            |mechanism: &str| gun_file_with("Gun Mk. 1", "0.05", &[("Probe", "700.0")], mechanism);
        load(belt(r#"Automatic(cyclic_rpm: (600.0, Tuned("x")))"#))
            .expect("the fixture belt gun loads");
        for (text, word) in [
            (
                r#"(names: [], caliber: (0.05, Tuned("x")),
                    ammunition: { "Probe": (muzzle_speed: (700.0, Tuned("x"))) },
                    mechanism: Single, report_clips: [])"#
                    .to_owned(),
                "names",
            ),
            (
                gun_file("Gun \u{864E}", "0.05", &[("Probe", "700.0")]),
                "name",
            ),
            (
                gun_file("Gun Mk. 1", "0.0", &[("Probe", "700.0")]),
                "caliber",
            ),
            (
                gun_file("Gun Mk. 1", "NaN", &[("Probe", "700.0")]),
                "caliber",
            ),
            (gun_file("Gun Mk. 1", "0.05", &[]), "empty"),
            (gun_file("Gun Mk. 1", "0.05", &[("Nope", "700.0")]), "Nope"),
            (gun_file("Gun Mk. 1", "0.05", &[("Probe", "0.0")]), "speed"),
            (gun_file("Gun Mk. 1", "0.05", &[("Probe", "NaN")]), "speed"),
            (
                belt(r#"Automatic(cyclic_rpm: (0.0, Tuned("x")))"#),
                "cyclic_rpm",
            ),
            (
                belt(r#"Automatic(cyclic_rpm: (NaN, Tuned("x")))"#),
                "cyclic_rpm",
            ),
        ] {
            let err = load(text).unwrap_err();
            assert!(
                matches!(&err, GunError::Invalid { gun, reason }
                    if gun == "Gun" && reason.contains(word)),
                "{word}: {err}"
            );
        }
        for (text, word) in [
            (
                r#"(names: [("Gun Mk. 1", Tuned("x"))], caliber: (0.05, Tuned("x")),
                    ammunition: { "Probe": (muzzle_speed: 700.0) }, mechanism: Single,
                    report_clips: [])"#
                    .to_owned(),
                "provenance",
            ),
            (belt("Automatic(cyclic_rpm: 600.0)"), "provenance"),
            (
                r#"(names: [("Gun Mk. 1", Tuned("x"))], caliber: (0.05, Tuned("x")),
                    ammunition: { "Probe": (muzzle_speed: (700.0, Tuned("x"))),
                                  "Probe": (muzzle_speed: (710.0, Tuned("x"))) },
                    mechanism: Single, report_clips: [])"#
                    .to_owned(),
                "twice",
            ),
        ] {
            let err = load(text).unwrap_err();
            assert!(
                matches!(&err, GunError::Parse { gun, err } if gun == "Gun" && err.contains(word)),
                "{word}: {err}"
            );
        }
    }

    /// A lookup names what is wrong and never defaults: an unknown gun, a round no registry
    /// carries, a round the named gun does not chamber, and — by key alone — a round two guns
    /// chamber. A round fired through its gun carries that gun's speed for it, so two guns that
    /// chamber one round give it two speeds.
    #[test]
    fn a_lookup_names_the_gun_and_the_round_it_cannot_resolve() {
        let rounds = fixture_rounds(&["Probe", "Other"]);
        let guns = GunRegistry::from_files(
            &[
                (
                    "Long",
                    &gun_file(
                        "Long Mk. 1",
                        "0.05",
                        &[("Probe", "800.0"), ("Other", "700.0")],
                    ),
                ),
                (
                    "Short",
                    &gun_file("Short Mk. 1", "0.05", &[("Probe", "600.0")]),
                ),
            ],
            &rounds,
        )
        .expect("the fixture guns load");
        assert_eq!(guns.round("Long", "Probe").unwrap().muzzle_speed, 800.0);
        assert_eq!(guns.round("Short", "Probe").unwrap().muzzle_speed, 600.0);
        assert_eq!(guns.round_by_key("Other").unwrap().muzzle_speed, 700.0);
        assert!(matches!(
            guns.round("Nope", "Probe"),
            Err(GunError::Unknown { name, .. }) if name == "Nope"
        ));
        assert!(matches!(
            guns.round("Short", "Other"),
            Err(GunError::NotChambered { gun, round, .. }) if gun == "Short" && round == "Other"
        ));
        for err in [guns.round("Long", "Nope"), guns.round_by_key("Nope")] {
            assert!(
                matches!(&err, Err(GunError::Round(RoundError::Unknown { name, .. })) if name == "Nope"),
                "{err:?}"
            );
        }
        assert_eq!(
            guns.round_by_key("Probe"),
            Err(GunError::Ambiguous {
                round: "Probe".into(),
                guns: vec!["Long".into(), "Short".into()],
            })
        );
    }

    /// The embedded index (`build.rs`) names exactly the files on disk in `assets/rounds` and
    /// `assets/guns`, by stem and in byte order, each with the text on disk. The registries parse
    /// from the index alone, so a file the index missed would be a round or a gun that silently
    /// does not exist, and a stale text one that silently differs from its file.
    #[test]
    fn the_embedded_index_names_every_armament_file_on_disk() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for (index, directory, suffix) in [
            (
                crate::armaments_index::ROUND_FILES,
                "assets/rounds",
                ".round.ron",
            ),
            (crate::armaments_index::GUN_FILES, "assets/guns", ".gun.ron"),
        ] {
            let mut on_disk: Vec<(String, String)> = std::fs::read_dir(root.join(directory))
                .unwrap_or_else(|err| panic!("{directory}: {err}"))
                .map(|entry| entry.expect("a directory entry"))
                .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                .map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let stem = name
                        .strip_suffix(suffix)
                        .unwrap_or_else(|| panic!("{directory}/{name} is not a {suffix} file"))
                        .to_owned();
                    let text = std::fs::read_to_string(entry.path()).expect("a readable file");
                    (stem, text)
                })
                .collect();
            on_disk.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            let embedded: Vec<(String, String)> = index
                .iter()
                .map(|&(stem, text)| (stem.to_owned(), text.to_owned()))
                .collect();
            assert!(!embedded.is_empty(), "{directory} holds no file");
            assert_eq!(embedded, on_disk, "{directory}");
        }
    }
}
