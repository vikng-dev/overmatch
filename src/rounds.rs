//! The round registry (`assets/ballistics/rounds.ron`): every round a weapon can fire, keyed by
//! key. A weapon names its rounds by those keys ([`crate::spec::WeaponSpec::rounds`]); a round
//! states its display name, core material, nose geometry, mass, calibre, muzzle speed and crew line
//! rate, and optionally its penetrator's mass and calibre, each with its provenance
//! ([`crate::provenance`]). The key is the data's handle and is
//! never shown to a player; the display name is ([`RoundRegistry::name`]).
//!
//! Parsed once from the embedded file and validated at parse: a missing display name or one the
//! shipped font does not draw whole ([`crate::ui_font::drawable`]), a core that is not a substance
//! with material facts, a nose parameter outside its family's range, or a round outside the force
//! law's domain, or a penetrator heavier or wider than its round, is a load error naming the round.
//! What a round derives — its nose factor and its two bodies ([`RoundBody`]) — is computed here,
//! once. A round flies as its whole `body` and meets the plate as its `penetrator`: the mechanism
//! binds from the penetrator at spawn ([`crate::ballistics`]'s `Projectile`), synchronously and
//! from data alone; drag and the shell's view read the body.
//!
//! A round may carry a burst block naming a burst core type of the registry's `burst_cores`; its
//! burst is derived here, once ([`crate::ballistics::burst::Burst::derived`]), and rides the round
//! to the shot as the penetrator does. A round without the block is inert. The burst's story is
//! design doc §5a, "The burst".
//!
//! The target structure (rounds name a core material and author their nose as geometry) is
//! `.agents/scratch/damage-cam-2026-10-08/research/recommendation-solid-shot.md` Part 1 item 5.

use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};
use std::collections::BTreeMap;

use crate::ballistics::burst::{Burst, BurstInputs, BurstOption, BurstShape};
use crate::ballistics::{RoundBody, round_in_domain};
use crate::provenance::Sourced;
use crate::spec::NoseShape;
use crate::substances::{MaterialFacts, SubstanceRegistry};

/// The shipped registry, embedded so a round is in hand synchronously wherever it is bound
/// (ADR-0014: built from data, never from an asset).
const ROUNDS_RON: &str = include_str!("../assets/ballistics/rounds.ron");

/// One round, resolved and validated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Round {
    /// The whole round as it flies: mass (kg), calibre (m) and the nose factor DERIVED from the
    /// authored nose at load. Drag and the shell's view read it.
    pub(crate) body: RoundBody,
    /// What meets the plate: the force law, the disc, overmatch and the cone's hole read it. The
    /// body's nose factor; mass and calibre no greater than the body's. Equal to `body` for a
    /// round authored without a penetrator block.
    pub(crate) penetrator: RoundBody,
    /// m/s.
    pub(crate) muzzle_speed: f32,
    /// What the round's line does to a witness (crew): reference-mm of dose per metre of disc-mean
    /// chord through it (`ballistics::wound`). Finite and positive. Balance data, never read by the
    /// hard walk.
    pub(crate) wound: f32,
    /// The core's material facts. The rigid regime models every core as rigid and reads none of
    /// them; a regime that deforms or erodes the round reads them.
    pub(crate) core: MaterialFacts,
    /// The round's burst, derived from its burst block and its body steel (`core`'s density);
    /// `None` for an inert round.
    pub(crate) burst: Option<Burst>,
}

/// One round as authored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoundFile {
    /// What a player reads: the round's own designation.
    name: Sourced<String>,
    core: Sourced<String>,
    nose: Sourced<NoseShape>,
    mass: Sourced<f32>,
    caliber: Sourced<f32>,
    muzzle_speed: Sourced<f32>,
    wound: Sourced<f32>,
    /// The part of a sub-calibre round that meets the plate; absent, the whole round does.
    #[serde(default)]
    penetrator: Option<PenetratorFile>,
    /// The round's burst core and fuze; absent, the round is inert.
    #[serde(default)]
    burst: Option<BurstFile>,
}

/// A burst block as authored: the core type it names, the field it throws, the fuze's threshold
/// (reference-mm spent in one body) and delay (s), and the core's mass, the base parts' mass (kg)
/// and the cavity's diameter (m).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BurstFile {
    core_type: Sourced<String>,
    option: Sourced<BurstShape>,
    arming_spend: Sourced<f32>,
    delay: Sourced<f32>,
    core_mass: Sourced<f64>,
    base_mass: Sourced<f64>,
    cavity_diameter: Sourced<f64>,
}

/// A burst core type as authored: its density (kg/m³) and speed constant `K` (m/s).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreTypeFile {
    density: Sourced<f64>,
    speed_constant_mps: Sourced<f64>,
}

/// A burst core type, validated: density (kg/m³) and speed constant (m/s), each finite and
/// positive.
#[derive(Clone, Copy, Debug)]
struct CoreType {
    density: f64,
    speed_constant: f64,
}

/// A penetrator as authored: its mass (kg) and calibre (m). Its nose is the round's.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PenetratorFile {
    mass: Sourced<f32>,
    caliber: Sourced<f32>,
}

#[derive(Deserialize)]
#[serde(rename = "RoundRegistry", deny_unknown_fields)]
struct RegistryFile {
    rounds: KeyedMap<RoundFile>,
    /// Burst core types by key; a burst block names one.
    #[serde(default)]
    burst_cores: KeyedMap<CoreTypeFile>,
}

/// A table as authored, refusing a duplicated key at parse: serde's default map fill would keep
/// the last of two entries of one key with nobody told.
struct KeyedMap<T>(BTreeMap<String, T>);

impl<T> Default for KeyedMap<T> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

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
                            "{name:?} is declared twice — a weapon names its round, and a round \
                             its burst core type, by this key, so a duplicate is two answers for \
                             one name"
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
    /// The RON did not parse.
    Parse(String),
    /// A round parsed but cannot be fired: names the round and the reason.
    Invalid { round: String, reason: String },
    /// A lookup named a round the registry does not carry; carries the registry's vocabulary.
    Unknown { name: String, known: Vec<String> },
}

impl std::fmt::Display for RoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "rounds.ron failed to parse: {err}"),
            Self::Invalid { round, reason } => write!(f, "round {round:?}: {reason}"),
            Self::Unknown { name, known } => write!(
                f,
                "unknown round {name:?} — assets/ballistics/rounds.ron declares [{}]",
                known.join(", ")
            ),
        }
    }
}

/// Resolve one authored round against `substances` and the burst core types `cores` into the round
/// and its display name, or say why it cannot be fired or shown.
fn resolved(
    file: RoundFile,
    substances: &SubstanceRegistry,
    cores: &BTreeMap<String, CoreType>,
) -> Result<(Round, String), String> {
    let name = file.name.0;
    if crate::ui_font::drawable(&name).as_deref() != Some(name.as_str()) {
        return Err(format!(
            "name {name:?} is not drawn whole by the shipped font — a round's display name is \
             what `killcam::card_lines` draws"
        ));
    }
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
    let nose = file.nose.0;
    if let Some((parameter, value, false)) = nose.parameter() {
        return Err(format!("nose {parameter} is out of range (got {value})"));
    }
    let body = RoundBody {
        mass: file.mass.0,
        caliber: file.caliber.0,
        nose_factor: nose.factor(),
    };
    let muzzle_speed = file.muzzle_speed.0;
    round_in_domain(body, muzzle_speed).map_err(|reason| {
        format!("the round (nose {nose:?}) is outside the capability law's domain: {reason}")
    })?;
    let sub_calibre = file.penetrator.is_some();
    let penetrator = match file.penetrator {
        None => body,
        Some(penetrator) => {
            let penetrator = RoundBody {
                mass: penetrator.mass.0,
                caliber: penetrator.caliber.0,
                nose_factor: body.nose_factor,
            };
            round_in_domain(penetrator, muzzle_speed).map_err(|reason| {
                format!("the penetrator is outside the capability law's domain: {reason}")
            })?;
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
    let wound = file.wound.0;
    if !wound.is_finite() || wound <= 0.0 {
        return Err(format!(
            "`wound`, the crew line rate, must be finite and positive (got {wound}) — it is the \
             dose per metre of chord the round's line deals a witness"
        ));
    }
    let burst = match file.burst {
        None => None,
        Some(_) if sub_calibre => {
            return Err(
                "a round with a penetrator block carries no burst block — the burst's parts are \
                 derived from a full-calibre body"
                    .into(),
            );
        }
        Some(block) => {
            let key = block.core_type.0;
            let core_type = cores.get(&key).ok_or_else(|| {
                format!(
                    "burst: unknown core type {key:?} — `burst_cores` declares [{}]",
                    cores.keys().cloned().collect::<Vec<_>>().join(", ")
                )
            })?;
            let inputs = BurstInputs {
                shape: block.option.0,
                arming_spend: block.arming_spend.0,
                delay: block.delay.0,
                core_mass: block.core_mass.0,
                core_density: core_type.density,
                speed_constant: core_type.speed_constant,
                base_mass: block.base_mass.0,
                cavity_diameter: block.cavity_diameter.0,
            };
            let burst = Burst::derived(body, core.density, &inputs)
                .map_err(|reason| format!("burst: {reason}"))?;
            if let BurstOption::Ring { nose } = burst.option {
                round_in_domain(nose, muzzle_speed).map_err(|reason| {
                    format!(
                        "burst: the nose piece is outside the capability law's domain: {reason}"
                    )
                })?;
            }
            Some(burst)
        }
    };
    Ok((
        Round {
            body,
            penetrator,
            muzzle_speed,
            wound,
            core,
            burst,
        },
        name,
    ))
}

/// The authored burst core types, each refused unless its density and speed constant are finite
/// and positive.
fn core_types(file: KeyedMap<CoreTypeFile>) -> Result<BTreeMap<String, CoreType>, RoundError> {
    file.0
        .into_iter()
        .map(|(key, core)| {
            let core_type = CoreType {
                density: core.density.0,
                speed_constant: core.speed_constant_mps.0,
            };
            for (field, value) in [
                ("density", core_type.density),
                ("speed_constant_mps", core_type.speed_constant),
            ] {
                if !value.is_finite() || value <= 0.0 {
                    return Err(RoundError::Parse(format!(
                        "burst core type {key:?}: {field} must be finite and positive (got \
                         {value})"
                    )));
                }
            }
            Ok((key, core_type))
        })
        .collect()
}

/// The parsed, validated `rounds.ron`.
#[derive(Clone, Debug)]
pub(crate) struct RoundRegistry {
    /// Each round and its display name, by key.
    rounds: BTreeMap<String, (Round, String)>,
}

impl RoundRegistry {
    /// Parse a registry from RON text, resolving every round's core against `substances`.
    pub(crate) fn from_ron(text: &str, substances: &SubstanceRegistry) -> Result<Self, RoundError> {
        let file: RegistryFile =
            ron::de::from_str(text).map_err(|err| RoundError::Parse(err.to_string()))?;
        let cores = core_types(file.burst_cores)?;
        let rounds = file
            .rounds
            .0
            .into_iter()
            .map(|(name, round)| match resolved(round, substances, &cores) {
                Ok(resolved) => Ok((name, resolved)),
                Err(reason) => Err(RoundError::Invalid {
                    round: name,
                    reason,
                }),
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        Ok(Self { rounds })
    }

    /// The shipped registry, parsed once against the shipped substances. Panics on a malformed
    /// file: a broken authored contract is a build defect, as for the substance registry.
    pub(crate) fn shipped() -> &'static Self {
        static REGISTRY: std::sync::LazyLock<RoundRegistry> = std::sync::LazyLock::new(|| {
            RoundRegistry::from_ron(ROUNDS_RON, SubstanceRegistry::shipped_static())
                .unwrap_or_else(|err| panic!("rounds: embedded rounds.ron is unusable: {err}"))
        });
        &REGISTRY
    }

    /// The round named `name`; a hard error naming the name and the vocabulary, never a default.
    pub(crate) fn get(&self, name: &str) -> Result<Round, RoundError> {
        self.entry(name).map(|(round, _)| *round)
    }

    /// The display name of the round keyed `name`; an error as [`Self::get`]'s, never the key.
    pub(crate) fn name(&self, name: &str) -> Result<&str, RoundError> {
        self.entry(name).map(|(_, shown)| shown.as_str())
    }

    /// Every round's key, in key order.
    pub(crate) fn keys(&self) -> impl Iterator<Item = &str> {
        self.rounds.keys().map(String::as_str)
    }

    fn entry(&self, name: &str) -> Result<&(Round, String), RoundError> {
        self.rounds.get(name).ok_or_else(|| RoundError::Unknown {
            name: name.to_string(),
            known: self.rounds.keys().cloned().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fields` as authored round fields, each with a fixture provenance.
    fn authored(fields: &[(&str, &str)]) -> String {
        fields
            .iter()
            .map(|(field, value)| format!(r#"{field}: ({value}, Tuned("fixture"))"#))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// A registry of one round whose fields are `fields`, each authored with a fixture provenance.
    fn one(fields: &[(&str, &str)]) -> Result<RoundRegistry, RoundError> {
        RoundRegistry::from_ron(
            &format!(r#"(rounds: {{ "Probe": ({}) }})"#, authored(fields)),
            SubstanceRegistry::shipped_static(),
        )
    }

    /// The fixture round with a penetrator block of `mass` (kg) and `caliber` (m).
    fn with_penetrator(mass: &str, caliber: &str) -> Result<RoundRegistry, RoundError> {
        RoundRegistry::from_ron(
            &format!(
                r#"(rounds: {{ "Probe": ({}, penetrator: Some((mass: ({mass}, Tuned("fixture")),
                    caliber: ({caliber}, Tuned("fixture"))))) }})"#,
                authored(&FIXTURE)
            ),
            SubstanceRegistry::shipped_static(),
        )
    }

    /// The fixture round's fields: fixture data, not a shipped round.
    const FIXTURE: [(&str, &str); 7] = [
        ("name", r#""Probe Mk. 1""#),
        ("core", r#""GunSteel""#),
        ("nose", "Ogive(crh: 1.0)"),
        ("mass", "5.0"),
        ("caliber", "0.05"),
        ("muzzle_speed", "700.0"),
        ("wound", "20.0"),
    ];

    fn with(field: &str, value: &str) -> Result<RoundRegistry, RoundError> {
        let fields = FIXTURE
            .iter()
            .map(|&(name, default)| (name, if name == field { value } else { default }))
            .collect::<Vec<_>>();
        one(&fields)
    }

    /// Every shipped round loads, and each binds a body inside the force law's domain with a core
    /// that states its facts, and a display name of its own the font draws whole.
    #[test]
    fn the_shipped_rounds_load() {
        let registry = RoundRegistry::shipped();
        assert!(!registry.rounds.is_empty());
        for (name, (round, shown)) in &registry.rounds {
            round_in_domain(round.body, round.muzzle_speed)
                .unwrap_or_else(|reason| panic!("{name}: {reason}"));
            round_in_domain(round.penetrator, round.muzzle_speed)
                .unwrap_or_else(|reason| panic!("{name} penetrator: {reason}"));
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
            .copied()
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
            .copied()
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
        let whole = with("mass", "5.0").unwrap().get("Probe").unwrap();
        for (penetrator, body) in [
            (whole.penetrator.mass, whole.body.mass),
            (whole.penetrator.caliber, whole.body.caliber),
            (whole.penetrator.nose_factor, whole.body.nose_factor),
        ] {
            assert_eq!(penetrator.to_bits(), body.to_bits());
        }

        let cored = with_penetrator("1.25", "0.02")
            .unwrap()
            .get("Probe")
            .unwrap();
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

    /// A penetrator is part of the round it flies in: one heavier or wider than its round, or one
    /// outside the force law's domain at the round's muzzle speed, is refused at parse naming the
    /// round and the field; one as heavy and as wide as its round loads. The march reads the
    /// penetrator at the plate and never re-checks it.
    #[test]
    fn a_penetrator_outside_its_round_or_the_law_s_domain_is_refused() {
        for (mass, caliber, field) in [
            ("5.01", "0.02", "mass"),
            ("1.25", "0.0501", "caliber"),
            ("0.0", "0.02", "mass"),
            ("1.25", "NaN", "caliber"),
        ] {
            let err = with_penetrator(mass, caliber).unwrap_err();
            assert!(
                matches!(&err, RoundError::Invalid { round, reason }
                    if round == "Probe" && reason.contains("penetrator") && reason.contains(field)),
                "({mass}, {caliber}): {err}"
            );
        }
        let flush = with_penetrator("5.0", "0.05")
            .unwrap()
            .get("Probe")
            .unwrap();
        assert_eq!(flush.penetrator, flush.body);

        let stray = format!(
            r#"(rounds: {{ "Probe": ({}, penetrator: Some((mass: (1.0, Tuned("x")),
                caliber: (0.02, Tuned("x")), nose: (Flat, Tuned("x"))))) }})"#,
            authored(&FIXTURE)
        );
        assert!(
            RoundRegistry::from_ron(&stray, SubstanceRegistry::shipped_static()).is_err(),
            "a penetrator states no nose of its own"
        );
    }

    /// The fixture round with a burst block naming core type `core_type`, and a penetrator block
    /// when `penetrator`. The block's numbers and the one core type are fixture data.
    fn with_burst(core_type: &str, penetrator: bool) -> Result<RoundRegistry, RoundError> {
        let penetrator = if penetrator {
            r#", penetrator: Some((mass: (1.0, Tuned("x")), caliber: (0.02, Tuned("x"))))"#
        } else {
            ""
        };
        RoundRegistry::from_ron(
            &format!(
                r#"(rounds: {{ "Probe": ({}{penetrator}, burst: Some((
                    core_type: ({core_type:?}, Tuned("x")), option: (Ring, Tuned("x")),
                    arming_spend: (35.0, Tuned("x")), delay: (0.012, Tuned("x")),
                    core_mass: (0.02, Tuned("x")), base_mass: (0.1, Tuned("x")),
                    cavity_diameter: (0.02, Tuned("x"))))) }},
                burst_cores: {{ "Fixture core": (density: (1650.0, Tuned("x")),
                    speed_constant_mps: (2680.0, Tuned("x"))) }})"#,
                authored(&FIXTURE)
            ),
            SubstanceRegistry::shipped_static(),
        )
    }

    /// A burst block rides its round as the burst derived from the round's body, its body steel's
    /// density and the core type it names; a round without one is inert. A block naming no
    /// declared core type, or on a sub-calibre round, is refused naming the round, never read as
    /// inert: a mutant that drops the block, or derives it from another density, fails here.
    #[test]
    fn a_burst_block_rides_its_round_or_is_refused() {
        let inert = with("mass", "5.0").unwrap().get("Probe").unwrap();
        assert_eq!(inert.burst, None);
        let fused = with_burst("Fixture core", false)
            .unwrap()
            .get("Probe")
            .unwrap();
        let inputs = BurstInputs {
            shape: BurstShape::Ring,
            arming_spend: 35.0,
            delay: 0.012,
            core_mass: 0.02,
            core_density: 1650.0,
            speed_constant: 2680.0,
            base_mass: 0.1,
            cavity_diameter: 0.02,
        };
        assert_eq!(
            fused.burst,
            Some(Burst::derived(fused.body, fused.core.density, &inputs).unwrap())
        );
        for (core_type, penetrator, word) in [
            ("Nope", false, "core type"),
            ("Fixture core", true, "penetrator"),
        ] {
            let err = with_burst(core_type, penetrator).unwrap_err();
            assert!(
                matches!(&err, RoundError::Invalid { round, reason }
                    if round == "Probe" && reason.contains(word)),
                "{core_type}, {penetrator}: {err}"
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

    /// A value authored without provenance, a duplicated round and an unknown field do not load.
    #[test]
    fn malformed_files_do_not_load() {
        let substances = SubstanceRegistry::shipped_static();
        let bare = r#"(rounds: { "Probe": (name: ("Probe", Tuned("fixture")),
            core: ("GunSteel", Tuned("fixture")),
            nose: (Flat, Tuned("fixture")), mass: 5.0, caliber: (0.05, Tuned("fixture")),
            muzzle_speed: (700.0, Tuned("fixture")), wound: (20.0, Tuned("fixture"))) })"#;
        let err = RoundRegistry::from_ron(bare, substances).unwrap_err();
        assert!(err.to_string().contains("provenance"), "{err}");

        let round = authored(&FIXTURE);
        let twice = format!(r#"(rounds: {{ "Probe": ({round}), "Probe": ({round}) }})"#);
        let err = RoundRegistry::from_ron(&twice, substances).unwrap_err();
        assert!(err.to_string().contains("twice"), "{err}");

        let stray = format!(r#"(rounds: {{ "Probe": ({round}, hardness: (3.0, Tuned("x"))) }})"#);
        assert!(RoundRegistry::from_ron(&stray, substances).is_err());
    }

    /// Each nose family derives its factor by its own formula (a hemisphere and a half-calibre
    /// cone are `½`, DERIVED; a flat face `1`), a loaded round's body carries the factor DERIVED
    /// from its authored nose, and a parameter outside the family's range is a load error naming
    /// the round.
    #[test]
    fn nose_shapes_derive_their_factor_and_reject_out_of_range_parameters() {
        let loaded = with("mass", "5.0").unwrap().get("Probe").unwrap();
        assert_eq!(
            loaded.body.nose_factor,
            NoseShape::Ogive { crh: 1.0 }.factor()
        );
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
            let err = with("nose", nose).unwrap_err().to_string();
            assert!(
                err.contains("Probe") && err.contains("nose"),
                "{nose}: {err}"
            );
        }
    }

    /// Extreme FINITE nose parameters: a round is accepted exactly when its derived factor and the
    /// capability law's results stay in their finite valid domain, and an accepted factor is the
    /// family's own formula, not an overflowed intermediate.
    #[test]
    fn extreme_nose_parameters_are_accepted_only_inside_the_law_s_domain() {
        let accepted = |nose: &str| match with("nose", nose) {
            Ok(_) => true,
            Err(err) => {
                let err = err.to_string();
                assert!(
                    err.contains("Probe") && err.contains("nose"),
                    "{nose}: {err}"
                );
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
