//! The round registry: every round, one file each (`assets/rounds/<key>.round.ron`, the file stem
//! its key), gun-independent, resolved against the round-type table
//! (`assets/ballistics/round_types.ron`) and the substance registry. The key is the data's handle
//! and is never shown to a player; the display name is ([`RoundRegistry::name`]).
//!
//! A round has no muzzle speed of its own. [`RoundRegistry::get`] gives its [`RoundFacts`], and the
//! gun that fires it gives it a speed ([`crate::guns`], through [`RoundFacts::fired_at`]): the
//! resulting [`Round`] is the record the laws and the shot read, and a reader takes it from the gun
//! registry, never from here. Entry points: [`RoundRegistry::shipped`] (the round files embedded by
//! `build.rs`'s index, [`crate::armaments_index::ROUND_FILES`], parsed once), `get`, `name` and
//! `keys`.
//!
//! # A round file
//!
//! `name` is the round's historical designation, what a player reads; `round_type` a key of the
//! round-type table; `core` a substance with material facts; `mass` (kg) and `caliber` (m) the whole
//! round's; `wound` its crew line rate (ADR-0040): reference-mm of dose per metre of disc-mean chord
//! the shell cuts through a crew box, balance data the hard walk never reads, scaled from the 30 HP
//! crew pool (3 reference-mm, `ballistics::HP_PER_REFERENCE_MM`) so that a rate of `3 / L` empties
//! the pool at a chord of `L` metres. Optionally:
//!
//! - `nose: Some((NoseShape, Kind))`: the round's own hit nose, for a round documented to differ
//!   from its type's.
//! - `penetrator: Some((mass, caliber))`: the part of a sub-calibre round that meets the plate, its
//!   nose the round's. The round is sized and shown as its `flying` body and meets the plate as its
//!   `penetrator` (the force law, the disc, overmatch and the cone's hole read it); the mechanism
//!   binds from the penetrator at spawn ([`crate::ballistics`]'s `Projectile`), synchronously and
//!   from data alone. A round without the block meets the plate whole.
//! - `fuze: Some((position, trigger, modes, default_mode))`: what sets the burst off
//!   ([`fuze`]). `position` is `Nose` or `Base`. `trigger` is `SpendExit(arming_spend: …)`, the
//!   reference-mm spent in one body and then left, or `Impact(sensitivity_n: …)`, the resisting
//!   force (N) at contact. `modes` is the ordered list of `(name, delay_s)` the crew picks among,
//!   `default_mode` the name of the one a round is loaded with. Every value is sourced.
//! - `burst: Some((core_substance, core_mass, base_mass, cavity_diameter))`: a substance-table key
//!   whose row states burst facts, the core's and the base parts' masses (kg) and the cavity's
//!   diameter (m). Every burst is a ring. The burst is derived here, once, at the default mode
//!   ([`crate::ballistics::burst::Burst::derived`]), and rides the round to the shot; its story is
//!   design doc §5a, "The burst". A round without the block is inert. A burst and a fuze are
//!   authored together.
//!
//! # How a round flies
//!
//! Drag reads the round type's flight nose, never the round's hit nose: a ballistic-capped type
//! flies on a pointed nose and meets the plate with a blunt one. The flying body is the whole round,
//! or the penetrator where the type's carrier is discarded in flight; [`Round::drag_k`] is
//! [`crate::ballistics::drag_form`] of the flight nose × that body's calibre² / mass, DERIVED at
//! load. The shell and the sight's range table both read it.
//!
//! Every value is authored `(value, Kind("source or reason"))` ([`crate::provenance`]); a value
//! without one does not load. "Q7" in a provenance is the fixture-materials recommendation in
//! `.agents/scratch/damage-cam-2026-10-08/research/hardness-anchor-free.md`, which collected no
//! munition data: the rounds are labelled fixtures.
//!
//! # What loads
//!
//! Each a load error naming the round or the type: a display name the shipped font does not draw
//! whole ([`crate::ui_font::drawable`]); a round type the table does not declare; a nose parameter
//! outside its family's range, in a type's either nose or a round's override; a core that is not a
//! substance with material facts; a penetrator heavier or wider than its round; a penetrator block
//! on a round of a type without a carrier, or none on one with a carrier; a crew line rate that is
//! not finite and positive; a burst block on a sub-calibre round, or one naming a substance without
//! burst facts. The force law's domain needs a speed, so a gun's ammunition table checks it
//! ([`RoundFacts::fired_at`]).

use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};
use std::collections::BTreeMap;

use crate::ballistics::burst::{Burst, BurstInputs};
use crate::ballistics::{RoundBody, round_in_domain};
use crate::provenance::Sourced;
use crate::spec::NoseShape;
use crate::substances::{MaterialFacts, SubstanceRegistry};

pub(crate) mod fuze;

/// The shipped round-type table, embedded as the round files are, so a round is in hand
/// synchronously wherever it is bound (ADR-0014: built from data, never from an asset).
const ROUND_TYPES_RON: &str = include_str!("../assets/ballistics/round_types.ron");

/// One round fired from one gun, resolved and validated: the record the laws read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Round {
    /// The game's damage model: the launched assembly, including any carrier. Mass (kg), calibre
    /// (m) and the nose factor DERIVED from its hit nose at load. Gun recoil reads this mass before
    /// a discarded carrier leaves the assembly.
    pub(crate) body: RoundBody,
    /// The flying body: the assembly for a kept carrier, the penetrator for a discarded one.
    /// Drag, the shell's view and the momentum a hit delivers read it.
    pub(crate) flying: RoundBody,
    /// What meets the plate: the force law, the disc, overmatch and the cone's hole read it. The
    /// body's nose factor; mass and calibre no greater than the body's. Equal to `body` for a
    /// round authored without a penetrator block.
    pub(crate) penetrator: RoundBody,
    /// m/s: the speed the firing gun's ammunition table gives this round.
    pub(crate) muzzle_speed: f32,
    /// The quadratic-drag coefficient (1/m) the round flies on, DERIVED at load from its type's
    /// flight nose and its flying body (module doc, "How a round flies").
    pub(crate) drag_k: f32,
    /// What the round's line does to a witness (crew): reference-mm of dose per metre of disc-mean
    /// chord through it (`ballistics::wound`). Finite and positive. Balance data, never read by the
    /// hard walk.
    pub(crate) wound: f32,
    /// The core's material facts. The rigid regime models every core as rigid and reads none of
    /// them; a regime that deforms or erodes the round reads them.
    pub(crate) core: MaterialFacts,
    /// The round's burst, derived from its burst block and its body steel (`core`'s density);
    /// `None` for an inert round. Its trigger and delay are the fuze mode `fuze_mode`'s.
    pub(crate) burst: Option<Burst>,
    /// The index of the fuze mode the burst is set to, in the round's authored `modes`; the
    /// file's `default_mode` until [`crate::tank::WeaponRound::in_mode`] picks another. 0 for an
    /// inert round.
    pub(crate) fuze_mode: u8,
}

/// One round as its file and its type resolve it, before a gun gives it a muzzle speed: its
/// authored assembly and every field of [`Round`] but speed, validated as far as no speed is needed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RoundFacts {
    /// The hit nose the factor is derived from; a domain refusal names it.
    nose: NoseShape,
    /// The authored assembly, including any carrier, for domain validation.
    body: RoundBody,
    flying: RoundBody,
    penetrator: RoundBody,
    drag_k: f32,
    wound: f32,
    core: MaterialFacts,
    pub(crate) burst: Option<Burst>,
    pub(crate) fuze: Option<fuze::Spec>,
}

impl RoundFacts {
    /// The round fired at `muzzle_speed` (m/s), or why the capability law cannot state it there:
    /// the body, the penetrator and a burst's nose piece each inside the law's domain at that speed
    /// ([`round_in_domain`]), so a round outside it never reaches the march.
    pub(crate) fn fired_at(&self, muzzle_speed: f32) -> Result<Round, String> {
        round_in_domain(self.body, muzzle_speed).map_err(|reason| {
            format!(
                "the round (nose {:?}) is outside the capability law's domain: {reason}",
                self.nose
            )
        })?;
        round_in_domain(self.penetrator, muzzle_speed).map_err(|reason| {
            format!("the penetrator is outside the capability law's domain: {reason}")
        })?;
        if let Some(Burst { nose, .. }) = self.burst {
            round_in_domain(nose, muzzle_speed).map_err(|reason| {
                format!("burst: the nose piece is outside the capability law's domain: {reason}")
            })?;
        }
        Ok(Round {
            body: self.body,
            flying: self.flying,
            penetrator: self.penetrator,
            muzzle_speed,
            drag_k: self.drag_k,
            wound: self.wound,
            core: self.core,
            burst: self.burst,
            fuze_mode: self.fuze.as_ref().map_or(0, |f| f.default_mode),
        })
    }
}

/// One round as authored.
#[derive(Deserialize)]
#[serde(rename = "Round", deny_unknown_fields)]
struct RoundFile {
    /// What a player reads: the round's own designation.
    name: Sourced<String>,
    /// A key of the round-type table.
    round_type: Sourced<String>,
    core: Sourced<String>,
    /// The round's own hit nose; absent, its type's.
    #[serde(default)]
    nose: Option<Sourced<NoseShape>>,
    mass: Sourced<f32>,
    caliber: Sourced<f32>,
    wound: Sourced<f32>,
    /// The part of a sub-calibre round that meets the plate; absent, the whole round does.
    #[serde(default)]
    penetrator: Option<PenetratorFile>,
    /// The round's burst core; absent, the round is inert.
    #[serde(default)]
    burst: Option<BurstFile>,
    /// What sets the burst off; authored with the burst and only with it.
    #[serde(default)]
    fuze: Option<fuze::File>,
}

/// A burst block as authored: the substance its core is made of, the core's mass, the base parts'
/// mass (kg) and the cavity's diameter (m). It names no shape: every burst is a ring. What sets it
/// off is the round's fuze block.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BurstFile {
    core_substance: Sourced<String>,
    core_mass: Sourced<f64>,
    base_mass: Sourced<f64>,
    cavity_diameter: Sourced<f64>,
}

/// A penetrator as authored: its mass (kg) and calibre (m). Its nose is the round's.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PenetratorFile {
    mass: Sourced<f32>,
    caliber: Sourced<f32>,
}

/// What carries a round's penetrator to the plate. A type has a carrier exactly when its rounds
/// author a penetrator block; drag reads which body flies.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
enum Carrier {
    /// None: the round meets the plate whole.
    None,
    /// The penetrator reaches the plate inside its carrier: the whole round flies.
    Kept,
    /// The carrier leaves the penetrator in flight: the penetrator flies.
    Discarded,
}

/// A round type as authored: its shape proxy and its carrier.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoundTypeFile {
    flight_nose: Sourced<NoseShape>,
    hit_nose: Sourced<NoseShape>,
    carrier: Sourced<Carrier>,
}

#[derive(Deserialize)]
#[serde(rename = "RoundTypes", deny_unknown_fields)]
struct RoundTypesFile {
    round_types: KeyedMap<RoundTypeFile>,
}

/// A round type, validated: the nose its rounds fly on, the hit nose they meet the plate with
/// unless they override it, and its carrier, which decides whether its rounds author a penetrator
/// block and which body flies.
#[derive(Clone, Copy, Debug)]
struct RoundType {
    flight_nose: NoseShape,
    hit_nose: NoseShape,
    carrier: Carrier,
}

/// A table as authored, refusing a duplicated key at parse: serde's default map fill would keep
/// the last of two entries of one key with nobody told.
pub(crate) struct KeyedMap<T>(pub(crate) BTreeMap<String, T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for KeyedMap<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for MapVisitor<T> {
            type Value = KeyedMap<T>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map of key to entry")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut out = BTreeMap::new();
                while let Some((name, entry)) = access.next_entry::<String, T>()? {
                    if out.contains_key(&name) {
                        return Err(de::Error::custom(format!(
                            "{name:?} is declared twice — the table is read by this key, so a \
                             duplicate is two answers for one name"
                        )));
                    }
                    out.insert(name, entry);
                }
                Ok(KeyedMap(out))
            }
        }

        deserializer.deserialize_map(MapVisitor(std::marker::PhantomData))
    }
}

/// Why a round registry, or a lookup in one, failed. Every arm names the offending text.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RoundError {
    /// A file did not parse: names the file.
    Parse { file: String, err: String },
    /// A round type parsed but is unusable: names the type and the reason.
    InvalidType { round_type: String, reason: String },
    /// A round parsed but cannot be fired: names the round and the reason.
    Invalid { round: String, reason: String },
    /// A lookup named a round the registry does not carry; carries the registry's vocabulary.
    Unknown { name: String, known: Vec<String> },
}

impl std::fmt::Display for RoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse { file, err } => write!(f, "{file} failed to parse: {err}"),
            Self::InvalidType { round_type, reason } => {
                write!(f, "round type {round_type:?}: {reason}")
            }
            Self::Invalid { round, reason } => write!(f, "round {round:?}: {reason}"),
            Self::Unknown { name, known } => write!(
                f,
                "unknown round {name:?} — assets/rounds/ holds [{}]",
                known.join(", ")
            ),
        }
    }
}

/// The nose parameter out of its family's range, as a refusal's reason, or `None` in range.
fn nose_out_of_range(field: &str, nose: NoseShape) -> Option<String> {
    match nose.parameter() {
        Some((parameter, value, false)) => {
            Some(format!("{field} {parameter} is out of range (got {value})"))
        }
        _ => None,
    }
}

/// The authored round-type table, each type refused unless both its noses are in range.
fn round_types(text: &str) -> Result<BTreeMap<String, RoundType>, RoundError> {
    let file: RoundTypesFile = ron::de::from_str(text).map_err(|err| RoundError::Parse {
        file: "assets/ballistics/round_types.ron".into(),
        err: err.to_string(),
    })?;
    file.round_types
        .0
        .into_iter()
        .map(|(key, row)| {
            for (field, nose) in [
                ("flight_nose", row.flight_nose.0),
                ("hit_nose", row.hit_nose.0),
            ] {
                if let Some(reason) = nose_out_of_range(field, nose) {
                    return Err(RoundError::InvalidType {
                        round_type: key,
                        reason,
                    });
                }
            }
            let round_type = RoundType {
                flight_nose: row.flight_nose.0,
                hit_nose: row.hit_nose.0,
                carrier: row.carrier.0,
            };
            Ok((key, round_type))
        })
        .collect()
}

/// Resolve one authored round against the round types `types` and `substances` into its facts and
/// its display name, or say why it cannot be fired or shown.
fn resolved(
    file: RoundFile,
    types: &BTreeMap<String, RoundType>,
    substances: &SubstanceRegistry,
) -> Result<(RoundFacts, String), String> {
    let name = file.name.0;
    if crate::ui_font::drawable(&name).as_deref() != Some(name.as_str()) {
        return Err(format!(
            "name {name:?} is not drawn whole by the shipped font — a round's display name is \
             what `killcam::card_lines` draws"
        ));
    }
    let type_key = file.round_type.0;
    let round_type = types.get(&type_key).ok_or_else(|| {
        format!(
            "round_type {type_key:?} is not a round type — assets/ballistics/round_types.ron \
             declares [{}]",
            types.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })?;
    let core_name = file.core.0;
    let core = substances
        .get(&core_name)
        .map_err(|err| format!("core: {err}"))?
        .hard()
        .and_then(|core| core.facts)
        .ok_or_else(|| {
            format!(
                "core {core_name:?} states no material facts — a round's core is a structural \
                 metal, and its facts are what a deforming or eroding regime reads"
            )
        })?;
    let nose = file.nose.map_or(round_type.hit_nose, |nose| nose.0);
    if let Some(reason) = nose_out_of_range("nose", nose) {
        return Err(reason);
    }
    let body = RoundBody {
        mass: file.mass.0,
        caliber: file.caliber.0,
        nose_factor: nose.factor(),
    };
    let sub_calibre = file.penetrator.is_some();
    if sub_calibre == (round_type.carrier == Carrier::None) {
        return Err(format!(
            "round type {type_key:?} has carrier {:?} and the round {} — a round meets the plate \
             whole exactly when its type has no carrier",
            round_type.carrier,
            if sub_calibre {
                "authors a penetrator block"
            } else {
                "authors no penetrator block"
            }
        ));
    }
    let penetrator = match file.penetrator {
        None => body,
        Some(penetrator) => {
            let penetrator = RoundBody {
                mass: penetrator.mass.0,
                caliber: penetrator.caliber.0,
                nose_factor: body.nose_factor,
            };
            for (field, part, whole) in [
                ("mass", penetrator.mass, body.mass),
                ("caliber", penetrator.caliber, body.caliber),
            ] {
                if part > whole {
                    return Err(format!(
                        "the penetrator's {field} {part} exceeds the round's {whole} — a \
                         penetrator is part of the round it flies in"
                    ));
                }
            }
            penetrator
        }
    };
    let flying = match round_type.carrier {
        Carrier::Discarded => penetrator,
        Carrier::None | Carrier::Kept => body,
    };
    let drag_k = crate::ballistics::drag_k(
        crate::ballistics::drag_form(round_type.flight_nose),
        flying.caliber,
        flying.mass,
    );
    let wound = file.wound.0;
    if !wound.is_finite() || wound <= 0.0 {
        return Err(format!(
            "`wound`, the crew line rate, must be finite and positive (got {wound}) — it is the \
             dose per metre of chord the round's line deals a witness"
        ));
    }
    let fuze = file
        .fuze
        .map(fuze::File::resolve)
        .transpose()
        .map_err(|reason| format!("fuze: {reason}"))?;
    let burst = match (file.burst, &fuze) {
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            return Err(
                "a burst block and a fuze block are authored together — the fuze is what \
                 sets the burst off, and a fuze with no burst sets off nothing"
                    .into(),
            );
        }
        (Some(_), Some(_)) if sub_calibre => {
            return Err(
                "a round with a penetrator block carries no burst block — the burst's parts are \
                 derived from a full-calibre body"
                    .into(),
            );
        }
        (Some(block), Some(fuze)) => {
            let key = block.core_substance.0;
            let facts = substances
                .get(&key)
                .map_err(|err| format!("burst: core_substance: {err}"))?
                .burst()
                .ok_or_else(|| {
                    format!(
                        "burst: core_substance {key:?} states no burst facts — a burst core is a \
                         substance whose row states `burst: Some(..)`"
                    )
                })?;
            let inputs = BurstInputs {
                trigger: fuze.trigger,
                delay: fuze.default_delay_s(),
                core_mass: block.core_mass.0,
                core_density: facts.density,
                speed_constant: facts.speed_constant,
                chemical_energy: facts.chemical_energy.unwrap_or(0.0),
                base_mass: block.base_mass.0,
                cavity_diameter: block.cavity_diameter.0,
            };
            Some(
                Burst::derived(body, core.density, &inputs)
                    .map_err(|reason| format!("burst: {reason}"))?,
            )
        }
    };
    Ok((
        RoundFacts {
            nose,
            body,
            flying,
            penetrator,
            drag_k,
            wound,
            core,
            burst,
            fuze,
        },
        name,
    ))
}

/// The parsed, validated round files.
#[derive(Clone, Debug)]
pub(crate) struct RoundRegistry {
    /// Each round's facts and display name, by key.
    rounds: BTreeMap<String, (RoundFacts, String)>,
}

impl RoundRegistry {
    /// Parse a registry from round files `files` (`(key, RON text)`), resolving each round against
    /// the round-type table `types` (RON text) and `substances`. A key given twice is refused.
    pub(crate) fn from_files(
        files: &[(&str, &str)],
        types: &str,
        substances: &SubstanceRegistry,
    ) -> Result<Self, RoundError> {
        let types = round_types(types)?;
        let mut rounds = BTreeMap::new();
        for &(key, text) in files {
            let invalid = |reason: String| RoundError::Invalid {
                round: key.to_owned(),
                reason,
            };
            if rounds.contains_key(key) {
                return Err(invalid(
                    "declared twice — a file's stem is its round's key, so a duplicate is two \
                     answers for one name"
                        .into(),
                ));
            }
            let file: RoundFile = ron::de::from_str(text).map_err(|err| RoundError::Parse {
                file: format!("assets/rounds/{key}.round.ron"),
                err: err.to_string(),
            })?;
            let round = resolved(file, &types, substances).map_err(invalid)?;
            rounds.insert(key.to_owned(), round);
        }
        Ok(Self { rounds })
    }

    /// The shipped registry: every embedded round file ([`crate::armaments_index::ROUND_FILES`]),
    /// parsed once against the shipped round types and substances. Panics on a malformed file: a
    /// broken authored contract is a build defect, as for the substance registry.
    pub(crate) fn shipped() -> &'static Self {
        static REGISTRY: std::sync::LazyLock<RoundRegistry> = std::sync::LazyLock::new(|| {
            RoundRegistry::from_files(
                crate::armaments_index::ROUND_FILES,
                ROUND_TYPES_RON,
                SubstanceRegistry::shipped_static(),
            )
            .unwrap_or_else(|err| panic!("rounds: the embedded round files are unusable: {err}"))
        });
        &REGISTRY
    }

    /// The facts of the round keyed `key`; a hard error naming the key and the vocabulary, never a
    /// default.
    pub(crate) fn get(&self, key: &str) -> Result<RoundFacts, RoundError> {
        self.entry(key).map(|(round, _)| round.clone())
    }

    /// The display name of the round keyed `key`; an error as [`Self::get`]'s, never the key.
    pub(crate) fn name(&self, key: &str) -> Result<&str, RoundError> {
        self.entry(key).map(|(_, shown)| shown.as_str())
    }

    /// Every round's key, in key order.
    pub(crate) fn keys(&self) -> impl Iterator<Item = &str> {
        self.rounds.keys().map(String::as_str)
    }

    fn entry(&self, key: &str) -> Result<&(RoundFacts, String), RoundError> {
        self.rounds.get(key).ok_or_else(|| RoundError::Unknown {
            name: key.to_string(),
            known: self.rounds.keys().cloned().collect(),
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The fixture round-type table: a full-calibre type and a sub-calibre one. Fixture data.
    pub(crate) const FIXTURE_TYPES: &str = r#"(round_types: {
        "Fixture": (flight_nose: (Ogive(crh: 1.0), Tuned("fixture")),
            hit_nose: (Ogive(crh: 1.0), Tuned("fixture")), carrier: (None, Tuned("fixture"))),
        "Fixture sub-calibre": (flight_nose: (Ogive(crh: 1.0), Tuned("fixture")),
            hit_nose: (Ogive(crh: 1.0), Tuned("fixture")), carrier: (Kept, Tuned("fixture"))),
    })"#;

    /// `fields` as authored round fields, each with a fixture provenance.
    fn authored(fields: &[(&str, &str)]) -> String {
        fields
            .iter()
            .map(|(field, value)| format!(r#"{field}: ({value}, Tuned("fixture"))"#))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The fixture round's fields: fixture data, not a shipped round.
    pub(crate) const FIXTURE: [(&str, &str); 6] = [
        ("name", r#""Probe Mk. 1""#),
        ("round_type", r#""Fixture""#),
        ("core", r#""GunSteel""#),
        ("mass", "5.0"),
        ("caliber", "0.05"),
        ("wound", "20.0"),
    ];

    /// The fixture round file, its fields `fields` authored, then `extra` verbatim.
    pub(crate) fn file(fields: &[(&str, &str)], extra: &str) -> String {
        format!("({}{extra})", authored(fields))
    }

    /// A registry of the one round `text`, keyed "Probe", over the fixture types.
    fn load(text: &str) -> Result<RoundRegistry, RoundError> {
        RoundRegistry::from_files(
            &[("Probe", text)],
            FIXTURE_TYPES,
            SubstanceRegistry::shipped_static(),
        )
    }

    /// A registry of one round whose fields are `fields`, each authored with a fixture provenance.
    fn one(fields: &[(&str, &str)]) -> Result<RoundRegistry, RoundError> {
        load(&file(fields, ""))
    }

    /// The fixture round with field `field` authored as `value`.
    fn with(field: &str, value: &str) -> Result<RoundRegistry, RoundError> {
        let fields = FIXTURE
            .iter()
            .map(|&(name, default)| (name, if name == field { value } else { default }))
            .collect::<Vec<_>>();
        one(&fields)
    }

    /// The fixture round, then `extra` verbatim.
    fn with_extra(extra: &str) -> Result<RoundRegistry, RoundError> {
        load(&file(&FIXTURE, extra))
    }

    /// The fixture round of the sub-calibre type, then `extra` verbatim.
    fn sub_calibre(extra: &str) -> Result<RoundRegistry, RoundError> {
        let fields = FIXTURE
            .iter()
            .map(|&(name, value)| {
                if name == "round_type" {
                    (name, r#""Fixture sub-calibre""#)
                } else {
                    (name, value)
                }
            })
            .collect::<Vec<_>>();
        load(&file(&fields, extra))
    }

    /// The fixture round of the sub-calibre type with a penetrator block of `mass` (kg) and
    /// `caliber` (m).
    fn with_penetrator(mass: &str, caliber: &str) -> Result<RoundRegistry, RoundError> {
        sub_calibre(&format!(
            r#", penetrator: Some((mass: ({mass}, Tuned("fixture")),
                caliber: ({caliber}, Tuned("fixture"))))"#
        ))
    }

    /// The fixture round's facts fired at the fixture speed, 700 m/s.
    fn fired(registry: Result<RoundRegistry, RoundError>) -> Result<Round, String> {
        registry
            .map_err(|err| err.to_string())?
            .get("Probe")
            .map_err(|err| err.to_string())?
            .fired_at(700.0)
    }

    /// Every shipped round loads, with a core that states its facts and a display name of its own
    /// the font draws whole; whether each is inside the force law's domain is its gun's to check
    /// (`guns`).
    #[test]
    fn the_shipped_rounds_load() {
        let registry = RoundRegistry::shipped();
        assert!(!registry.rounds.is_empty());
        for (name, (round, shown)) in &registry.rounds {
            assert!(
                round.penetrator.mass <= round.body.mass
                    && round.penetrator.caliber <= round.body.caliber,
                "{name}"
            );
            assert!(round.core.flow_strength > 0.0, "{name}");
            assert_eq!(registry.name(name).unwrap(), shown);
            assert_ne!(shown, name, "{name} shows its key");
            assert_eq!(
                crate::ui_font::drawable(shown).as_deref(),
                Some(shown.as_str())
            );
        }
    }

    /// A round without a display name does not load, and there is no fallback to its key; nor does
    /// one whose name is blank or not drawn whole by the shipped font. The fixture round's name is
    /// what [`RoundRegistry::name`] gives.
    #[test]
    fn a_round_without_a_drawable_name_does_not_load() {
        assert_eq!(
            with("mass", "5.0").unwrap().name("Probe").unwrap(),
            "Probe Mk. 1"
        );
        let unnamed: Vec<_> = FIXTURE
            .iter()
            .cloned()
            .filter(|(field, _)| *field != "name")
            .collect();
        let err = one(&unnamed).unwrap_err().to_string();
        assert!(err.contains("name"), "{err}");
        for name in [r#""""#, r#""   ""#, "\"8.8 cm \u{864E}\""] {
            let err = with("name", name).unwrap_err();
            assert!(
                matches!(&err, RoundError::Invalid { round, reason }
                    if round == "Probe" && reason.contains("name")),
                "{name}: {err}"
            );
        }
    }

    /// A round's crew line rate is finite and positive, refused naming the round otherwise; every
    /// shipped round states one.
    #[test]
    fn a_wound_rate_is_finite_and_positive() {
        for wound in ["0.0", "-5.0", "NaN", "inf"] {
            let err = with("wound", wound).unwrap_err();
            assert!(
                matches!(&err, RoundError::Invalid { round, reason }
                    if round == "Probe" && reason.contains("wound")),
                "{wound}: {err}"
            );
        }
        let unwounding: Vec<_> = FIXTURE
            .iter()
            .cloned()
            .filter(|(field, _)| *field != "wound")
            .collect();
        assert!(
            one(&unwounding).is_err(),
            "a round without a crew line rate must not load"
        );
        for name in RoundRegistry::shipped().keys() {
            let round = RoundRegistry::shipped().get(name).unwrap();
            assert!(round.wound.is_finite() && round.wound > 0.0, "{name}");
        }
    }

    /// A round authored without a penetrator block meets the plate as itself, bit for bit: every
    /// full-calibre round's record and digest rest on it. One with a block carries the block's mass
    /// and calibre and the round's nose, and flies as its own body still. A mutant that defaults
    /// the penetrator to anything but the body, or that reads the block into the body, fails here.
    #[test]
    fn a_penetrator_is_the_body_unless_authored() {
        let whole = fired(with("mass", "5.0")).unwrap();
        for (penetrator, body) in [
            (whole.penetrator.mass, whole.body.mass),
            (whole.penetrator.caliber, whole.body.caliber),
            (whole.penetrator.nose_factor, whole.body.nose_factor),
        ] {
            assert_eq!(penetrator.to_bits(), body.to_bits());
        }

        let cored = fired(with_penetrator("1.25", "0.02")).unwrap();
        assert_eq!(whole.body, cored.body, "the block must not move the body");
        assert_eq!(
            cored.penetrator,
            RoundBody {
                mass: 1.25,
                caliber: 0.02,
                nose_factor: whole.body.nose_factor,
            }
        );
    }

    /// A penetrator is part of the round it flies in: one heavier or wider than its round is
    /// refused at parse naming the round and the field, and one outside the force law's domain at
    /// the round's muzzle speed is refused where a gun fires it; one as heavy and as wide as its
    /// round loads. The march reads the penetrator at the plate and never re-checks it.
    #[test]
    fn a_penetrator_outside_its_round_or_the_law_s_domain_is_refused() {
        for (mass, caliber, field) in [("5.01", "0.02", "mass"), ("1.25", "0.0501", "caliber")] {
            let err = with_penetrator(mass, caliber).unwrap_err();
            assert!(
                matches!(&err, RoundError::Invalid { round, reason }
                    if round == "Probe" && reason.contains("penetrator") && reason.contains(field)),
                "({mass}, {caliber}): {err}"
            );
        }
        for (mass, caliber, field) in [("0.0", "0.02", "mass"), ("1.25", "NaN", "caliber")] {
            let err = fired(with_penetrator(mass, caliber)).unwrap_err();
            assert!(
                err.contains("penetrator") && err.contains(field),
                "({mass}, {caliber}): {err}"
            );
        }
        let flush = fired(with_penetrator("5.0", "0.05")).unwrap();
        assert_eq!(flush.penetrator, flush.body);

        let stray = r#", penetrator: Some((mass: (1.0, Tuned("x")),
            caliber: (0.02, Tuned("x")), nose: (Flat, Tuned("x"))))"#;
        let err = sub_calibre(stray).expect_err("a penetrator states no nose of its own");
        assert!(matches!(err, RoundError::Parse { .. }), "{err}");
    }

    /// The drag tests' round types: one flying on its blunt hit nose, one capped (a pointed flight
    /// nose over the same blunt hit nose), and a sub-calibre type of each carrier. Fixture data.
    const DRAG_TYPES: &str = r#"(round_types: {
        "Blunt": (flight_nose: (Ogive(crh: 1.0), Tuned("fixture")),
            hit_nose: (Ogive(crh: 1.0), Tuned("fixture")), carrier: (None, Tuned("fixture"))),
        "Capped": (flight_nose: (Ogive(crh: 4.0), Tuned("fixture")),
            hit_nose: (Ogive(crh: 1.0), Tuned("fixture")), carrier: (None, Tuned("fixture"))),
        "Kept": (flight_nose: (Ogive(crh: 1.0), Tuned("fixture")),
            hit_nose: (Ogive(crh: 1.0), Tuned("fixture")), carrier: (Kept, Tuned("fixture"))),
        "Discarded": (flight_nose: (Ogive(crh: 1.0), Tuned("fixture")),
            hit_nose: (Ogive(crh: 1.0), Tuned("fixture")), carrier: (Discarded, Tuned("fixture"))),
    })"#;

    /// The fixture round of type `round_type` over [`DRAG_TYPES`], of `mass` (kg) and `caliber`
    /// (m), then `extra` verbatim, fired at the fixture speed.
    pub(crate) fn of_type(round_type: &str, mass: &str, caliber: &str, extra: &str) -> Round {
        let round_type = format!("{round_type:?}");
        let fields = FIXTURE
            .iter()
            .map(|&(name, value)| match name {
                "round_type" => (name, round_type.as_str()),
                "mass" => (name, mass),
                "caliber" => (name, caliber),
                _ => (name, value),
            })
            .collect::<Vec<_>>();
        fired(RoundRegistry::from_files(
            &[("Probe", &file(&fields, extra))],
            DRAG_TYPES,
            SubstanceRegistry::shipped_static(),
        ))
        .unwrap()
    }

    /// A round's speed (m/s) after `range` metres of flat flight from `speed`, stepped by the
    /// shared flight kernel on its drag.
    fn speed_at(round: &Round, speed: f32, range: f32) -> f32 {
        let (mut position, mut velocity) = (0.0, bevy::math::Vec3::X * speed);
        while position < range {
            velocity = crate::ballistics::freeflight_step(velocity, round.drag_k, 1.0 / 64.0);
            position += velocity.x / 64.0;
        }
        velocity.length()
    }

    /// Drag reads a round type's flight nose and the plate its hit nose. Of two rounds of equal
    /// calibre and mass, the one whose type flies on a pointed nose keeps more speed at 1000 m than
    /// the one flying on a blunt nose, and both meet the plate with the same blunt hit nose. The
    /// sight's range table and the potency a round arrives with both rest on this. A mutant that
    /// drags on the hit nose, or meets the plate with the flight nose, fails here.
    #[test]
    fn a_pointed_flight_nose_keeps_more_speed_and_meets_the_plate_with_the_hit_nose() {
        let blunt = of_type("Blunt", "5.0", "0.05", "");
        let capped = of_type("Capped", "5.0", "0.05", "");
        let (blunt_speed, capped_speed) = (
            speed_at(&blunt, 700.0, 1000.0),
            speed_at(&capped, 700.0, 1000.0),
        );
        assert!(
            capped_speed > blunt_speed,
            "pointed {capped_speed} m/s vs blunt {blunt_speed} m/s at 1000 m"
        );
        assert_eq!(
            capped.penetrator, blunt.penetrator,
            "both meet the plate with the one blunt hit nose"
        );
    }

    /// A kept carrier flies whole and a discarded one leaves its penetrator flying alone: the
    /// discarded round drags as a full-calibre round of its penetrator's mass and calibre, the kept
    /// one as a full-calibre round of its own. A mutant that flies the whole round past a discarded
    /// carrier, or the core inside a kept one, fails here. The live shell's impulse and view must
    /// also read the flying body: a discarded carrier contributes neither mass nor calibre.
    #[test]
    fn a_discarded_carrier_leaves_the_penetrator_flying() {
        let core = r#", penetrator: Some((mass: (1.25, Tuned("fixture")),
            caliber: (0.02, Tuned("fixture"))))"#;
        let kept = of_type("Kept", "5.0", "0.05", core);
        let discarded = of_type("Discarded", "5.0", "0.05", core);
        assert_eq!(kept.drag_k, of_type("Blunt", "5.0", "0.05", "").drag_k);
        assert_eq!(
            discarded.drag_k,
            of_type("Blunt", "1.25", "0.02", "").drag_k
        );
        assert_ne!(kept.drag_k, discarded.drag_k);
        assert_eq!(
            discarded.body, kept.body,
            "launch recoil reads the assembly before either carrier leaves it"
        );
        for (round, expected) in [(kept, kept.body), (discarded, discarded.penetrator)] {
            let fire = crate::ballistics::FireShell::of_round(
                &round,
                bevy::math::Vec3::ZERO,
                bevy::math::Dir3::X,
                crate::spec::FireMechanism::Single,
                false,
                None,
                None,
            );
            assert_eq!(
                fire.mass, expected.mass,
                "only the flying mass delivers momentum"
            );
            assert_eq!(
                fire.caliber, expected.caliber,
                "the view sizes the flying body"
            );
        }
    }

    /// A round type's carrier decides whether its rounds author a penetrator block: a round meets
    /// the plate whole exactly when its type has no carrier. A full-calibre type's round with a
    /// block, and a sub-calibre type's round without one, are refused naming the round, so the
    /// type a round names can never contradict what the force law is handed at the plate.
    #[test]
    fn a_penetrator_block_is_authored_exactly_when_the_type_has_a_carrier() {
        let err = with_extra(
            r#", penetrator: Some((mass: (1.0, Tuned("x")), caliber: (0.02, Tuned("x"))))"#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, RoundError::Invalid { round, reason }
                if round == "Probe" && reason.contains("carrier") && reason.contains("None")),
            "{err}"
        );
        let err = with("round_type", r#""Fixture sub-calibre""#).unwrap_err();
        assert!(
            matches!(&err, RoundError::Invalid { round, reason }
                if round == "Probe" && reason.contains("carrier") && reason.contains("Kept")),
            "{err}"
        );
    }

    /// A round names a type the table declares, refused naming the round and the vocabulary
    /// otherwise; a type with either nose out of its family's range is refused naming the type;
    /// a carrier outside the closed set does not parse.
    #[test]
    fn a_round_type_is_declared_and_its_noses_in_range() {
        let err = with("round_type", r#""Shrapnel""#).unwrap_err();
        assert!(
            matches!(&err, RoundError::Invalid { round, reason }
                if round == "Probe" && reason.contains("Shrapnel") && reason.contains("Fixture")),
            "{err}"
        );
        let types = |flight: &str, hit: &str, carrier: &str| {
            format!(
                r#"(round_types: {{ "Odd": (flight_nose: ({flight}, Tuned("x")),
                    hit_nose: ({hit}, Tuned("x")), carrier: ({carrier}, Tuned("x"))) }})"#
            )
        };
        for (flight, hit, field) in [
            ("Ogive(crh: 0.4)", "Flat", "flight_nose"),
            ("Flat", "Cone(length_calibres: 0.0)", "hit_nose"),
        ] {
            let err = RoundRegistry::from_files(
                &[],
                &types(flight, hit, "None"),
                SubstanceRegistry::shipped_static(),
            )
            .unwrap_err();
            assert!(
                matches!(&err, RoundError::InvalidType { round_type, reason }
                    if round_type == "Odd" && reason.contains(field)),
                "{err}"
            );
        }
        let err = RoundRegistry::from_files(
            &[],
            &types("Flat", "Flat", "Sabot"),
            SubstanceRegistry::shipped_static(),
        )
        .unwrap_err();
        assert!(matches!(err, RoundError::Parse { .. }), "{err}");
    }

    /// The fixture round with a burst block naming substance `core`, on the sub-calibre type with
    /// a penetrator block when `penetrator`, over [`FIXTURE_SUBSTANCES`]. The block's numbers are
    /// fixture data.
    fn with_burst(core: &str, penetrator: bool) -> Result<RoundRegistry, RoundError> {
        let (round_type, block) = if penetrator {
            (
                r#""Fixture sub-calibre""#,
                r#", penetrator: Some((mass: (1.0, Tuned("x")), caliber: (0.02, Tuned("x"))))"#,
            )
        } else {
            (r#""Fixture""#, "")
        };
        let fields = FIXTURE
            .iter()
            .map(|&(name, value)| {
                (
                    name,
                    if name == "round_type" {
                        round_type
                    } else {
                        value
                    },
                )
            })
            .collect::<Vec<_>>();
        let extra = format!(
            r#"{block}, fuze: Some((position: (Base, Tuned("x")),
                trigger: (SpendExit(arming_spend: 35.0), Tuned("x")),
                modes: [(name: ("delay", Tuned("x")), delay_s: (0.012, Tuned("x")))],
                default_mode: ("delay", Tuned("x")))), burst: Some((
                core_substance: ({core:?}, Tuned("x")),

                core_mass: (0.02, Tuned("x")), base_mass: (0.1, Tuned("x")),
                cavity_diameter: (0.02, Tuned("x"))))"#
        );
        RoundRegistry::from_files(
            &[("Probe", &file(&fields, &extra))],
            FIXTURE_TYPES,
            &SubstanceRegistry::from_ron(FIXTURE_SUBSTANCES).unwrap(),
        )
    }

    /// A reference steel, one burst core worn by no volume, `"Fixture core"`, and one hard
    /// substance that is a burst core too, `"Fixture hard core"`: fixture data.
    const FIXTURE_SUBSTANCES: &str = r#"(substances: {
        "GunSteel": Hard(resistance: Reference, visual: "fixture",
            debris_half_angle_deg: (26.2, Tuned("x")), facts: Some((
                density: (7800.0, Tuned("x")), flow_strength: (1.18e9, Tuned("x")),
                youngs_modulus: (190.0e9, Tuned("x")), poisson_ratio: (0.29, Tuned("x"))))),
        "Fixture core": Unworn(burst: Some((density: (1650.0, Tuned("x")),
            speed_constant_mps: (2680.0, Tuned("x"))))),
        "Fixture hard core": Hard(resistance: Effective((200.0, Tuned("x"))), visual: "fixture",
            debris_half_angle_deg: (26.2, Tuned("x")), facts: None,
            burst: Some((density: (1650.0, Tuned("x")),
                speed_constant_mps: (2680.0, Tuned("x"))))),
    })"#;

    /// A burst block rides its round as the burst derived from the round's body, its body steel's
    /// density and the burst facts of the substance it names, whether that row is worn by no
    /// volume or is hard matter too; a round without one is inert. A block naming no substance,
    /// one naming a substance without burst facts, or one on a sub-calibre round, is refused naming
    /// the round, never read as inert: a mutant that drops the block, or derives it from another
    /// density, fails here. A block naming a burst shape does not parse: every burst is a ring.
    #[test]
    fn a_burst_block_rides_its_round_or_is_refused() {
        let inert = with("mass", "5.0").unwrap().get("Probe").unwrap();
        assert_eq!(inert.burst, None);
        let inputs = BurstInputs {
            trigger: fuze::Trigger::SpendExit { arming_spend: 35.0 },
            delay: 0.012,
            core_mass: 0.02,
            core_density: 1650.0,
            speed_constant: 2680.0,
            chemical_energy: 0.0,
            base_mass: 0.1,
            cavity_diameter: 0.02,
        };
        for core in ["Fixture core", "Fixture hard core"] {
            let fused = fired(with_burst(core, false)).unwrap();
            assert_eq!(
                fused.burst,
                Some(Burst::derived(fused.body, fused.core.density, &inputs).unwrap()),
                "{core}"
            );
        }
        let shaped = file(
            &FIXTURE,
            r#", burst: Some((core_substance: ("Fixture core", Tuned("x")),
                option: (Ring, Tuned("x")), arming_spend: (35.0, Tuned("x")),
                delay: (0.012, Tuned("x")), core_mass: (0.02, Tuned("x")),
                base_mass: (0.1, Tuned("x")), cavity_diameter: (0.02, Tuned("x")))))"#,
        );
        let err = RoundRegistry::from_files(
            &[("Probe", &shaped)],
            FIXTURE_TYPES,
            &SubstanceRegistry::from_ron(FIXTURE_SUBSTANCES).unwrap(),
        )
        .expect_err("a burst block names no shape");
        assert!(matches!(err, RoundError::Parse { .. }), "{err}");
        for (core, penetrator, word) in [
            ("Nope", false, "core_substance"),
            ("GunSteel", false, "burst facts"),
            ("Fixture core", true, "penetrator"),
        ] {
            let err = with_burst(core, penetrator).unwrap_err();
            assert!(
                matches!(&err, RoundError::Invalid { round, reason }
                    if round == "Probe" && reason.contains(word)),
                "{core}, {penetrator}: {err}"
            );
        }
    }

    /// An unknown name is an error naming the name and the vocabulary, never a default.
    #[test]
    fn an_unknown_round_is_an_error_naming_it() {
        let err = with("mass", "5.0").unwrap().get("Nope").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("Nope") && text.contains("Probe"), "{text}");
    }

    /// A core that is not a substance, or a substance with no facts, is refused naming the round.
    #[test]
    fn a_core_without_facts_is_refused() {
        for core in [r#""Unobtainium""#, r#""Flesh""#] {
            let err = with("core", core).unwrap_err();
            assert!(
                matches!(&err, RoundError::Invalid { round, .. } if round == "Probe"),
                "{core}: {err}"
            );
        }
    }

    /// A value authored without provenance, a key given twice and an unknown field do not load, nor
    /// does a muzzle speed: a round's speed is its gun's.
    #[test]
    fn malformed_files_do_not_load() {
        let bare = r#"(name: ("Probe", Tuned("fixture")), round_type: ("Fixture", Tuned("fixture")),
            core: ("GunSteel", Tuned("fixture")), mass: 5.0, caliber: (0.05, Tuned("fixture")),
            wound: (20.0, Tuned("fixture")))"#;
        let err = load(bare).unwrap_err();
        assert!(err.to_string().contains("provenance"), "{err}");

        let round = file(&FIXTURE, "");
        let err = RoundRegistry::from_files(
            &[("Probe", &round), ("Probe", &round)],
            FIXTURE_TYPES,
            SubstanceRegistry::shipped_static(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("twice"), "{err}");

        for stray in [
            r#", hardness: (3.0, Tuned("x"))"#,
            r#", muzzle_speed: (700.0, Tuned("x"))"#,
        ] {
            assert!(with_extra(stray).is_err(), "{stray}");
        }
    }

    /// A round's nose factor is DERIVED from its type's hit nose, or from its own `nose` where it
    /// authors one; each family derives its factor by its own formula (a hemisphere and a
    /// half-calibre cone are `½`, DERIVED; a flat face `1`); and a parameter outside the family's
    /// range is a load error naming the round.
    #[test]
    fn nose_shapes_derive_their_factor_and_reject_out_of_range_parameters() {
        let loaded = with("mass", "5.0").unwrap().get("Probe").unwrap();
        assert_eq!(
            loaded.body.nose_factor,
            NoseShape::Ogive { crh: 1.0 }.factor()
        );
        let own = with_extra(r#", nose: Some((Flat, Tuned("x")))"#)
            .unwrap()
            .get("Probe")
            .unwrap();
        assert_eq!(own.body.nose_factor, 1.0, "a round's own nose wins");
        for (shape, expected) in [
            (NoseShape::Ogive { crh: 0.5 }, 0.5),
            (
                NoseShape::Cap {
                    radius_calibres: 0.5,
                },
                0.5,
            ),
            (NoseShape::Flat, 1.0),
            (
                NoseShape::Cone {
                    length_calibres: 0.5,
                },
                0.5,
            ),
        ] {
            let got = shape.factor();
            assert!((got - expected).abs() < 1.0e-6, "{shape:?}: {got}");
        }
        for nose in [
            "Ogive(crh: 0.49)",
            "Cone(length_calibres: 0.0)",
            "Cap(radius_calibres: 0.4)",
            "Ogive(crh: NaN)",
        ] {
            let err = with_extra(&format!(r#", nose: Some(({nose}, Tuned("x")))"#))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("Probe") && err.contains("nose"),
                "{nose}: {err}"
            );
        }
    }

    /// Extreme FINITE nose parameters: a round is accepted exactly when its derived factor and the
    /// capability law's results stay in their finite valid domain at the fixture speed, and an
    /// accepted factor is the family's own formula, not an overflowed intermediate.
    #[test]
    fn extreme_nose_parameters_are_accepted_only_inside_the_law_s_domain() {
        let accepted = |nose: &str| match fired(with_extra(&format!(
            r#", nose: Some(({nose}, Tuned("x")))"#
        ))) {
            Ok(_) => true,
            Err(err) => {
                assert!(err.contains("nose"), "{nose}: {err}");
                false
            }
        };
        // DERIVED limits: ogive N → 1/(3ψ), cone N → 1/(4φ²), cap N → 1.
        for (nose, shape, limit) in [
            (
                "Ogive(crh: 1.0e20)",
                NoseShape::Ogive { crh: 1.0e20 },
                1.0 / 3.0e20,
            ),
            (
                "Cone(length_calibres: 1.0e18)",
                NoseShape::Cone {
                    length_calibres: 1.0e18,
                },
                0.25e-36,
            ),
            (
                "Cap(radius_calibres: 3.4028235e38)",
                NoseShape::Cap {
                    radius_calibres: f32::MAX,
                },
                1.0,
            ),
        ] {
            assert!(accepted(nose), "{nose} is inside the law's domain");
            let got = shape.factor() as f64;
            assert!(
                (got / limit - 1.0).abs() < 1.0e-6,
                "{shape:?}: factor {got} vs {limit}"
            );
        }
        for nose in [
            "Cone(length_calibres: 1.0e20)",
            "Ogive(crh: 3.4028235e38)",
            "Cone(length_calibres: 3.4028235e38)",
        ] {
            assert!(!accepted(nose), "{nose} derives a factor f32 cannot carry");
        }
    }
}
