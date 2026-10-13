//! The round registry (`assets/ballistics/rounds.ron`): every round a weapon can fire, keyed by
//! key. A weapon names its round by that key ([`crate::spec::WeaponSpec::round`]); the round states
//! its display name, core material, nose geometry, mass, calibre and muzzle speed, each with its
//! provenance ([`crate::provenance`]). The key is the data's handle and is never shown to a player;
//! the display name is ([`RoundRegistry::name`]).
//!
//! Parsed once from the embedded file and validated at parse: a missing display name or one the
//! shipped font does not draw whole ([`crate::ui_font::drawable`]), a core that is not a substance
//! with material facts, a nose parameter outside its family's range, or a round outside the force
//! law's domain is a load error naming the round. What a round derives — its nose factor and the body
//! the force law reads ([`RoundBody`]) — is computed here, once; the mechanism binds from that body
//! at spawn ([`crate::ballistics`]'s `Projectile`), synchronously and from data alone.
//!
//! The target structure (rounds name a core material and author their nose as geometry) is
//! `.agents/scratch/damage-cam-2026-10-08/research/recommendation-solid-shot.md` Part 1 item 5.

use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};
use std::collections::BTreeMap;

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
    /// What the force law reads: mass (kg), calibre (m) and the nose factor DERIVED from the
    /// authored nose at load.
    pub(crate) body: RoundBody,
    /// m/s.
    pub(crate) muzzle_speed: f32,
    /// The core's material facts. The rigid regime models every core as rigid and reads none of
    /// them; a regime that deforms or erodes the round reads them.
    pub(crate) core: MaterialFacts,
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
}

#[derive(Deserialize)]
#[serde(rename = "RoundRegistry", deny_unknown_fields)]
struct RegistryFile {
    rounds: RoundMap,
}

/// The round table as authored, refusing a duplicated name at parse: serde's default map fill
/// would keep the last of two rounds of one name with nobody told.
struct RoundMap(BTreeMap<String, RoundFile>);

impl<'de> Deserialize<'de> for RoundMap {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor;

        impl<'de> Visitor<'de> for MapVisitor {
            type Value = RoundMap;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map of round name to round")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut out = BTreeMap::new();
                while let Some((name, round)) = access.next_entry::<String, RoundFile>()? {
                    if out.contains_key(&name) {
                        return Err(de::Error::custom(format!(
                            "round {name:?} is declared twice — a weapon names its round by this \
                             key, so a duplicate is two answers for one name"
                        )));
                    }
                    out.insert(name, round);
                }
                Ok(RoundMap(out))
            }
        }

        deserializer.deserialize_map(MapVisitor)
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

/// Resolve one authored round against `substances` into the round and its display name, or say why
/// it cannot be fired or shown.
fn resolved(file: RoundFile, substances: &SubstanceRegistry) -> Result<(Round, String), String> {
    let name = file.name.0;
    if crate::ui_font::drawable(&name).as_deref() != Some(name.as_str()) {
        return Err(format!(
            "name {name:?} is not drawn whole by the shipped font — a round's display name is \
             what the kill card shows"
        ));
    }
    let core_name = file.core.0;
    let core = substances
        .get(&core_name)
        .map_err(|err| format!("core: {err}"))?
        .facts
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
    Ok((
        Round {
            body,
            muzzle_speed,
            core,
        },
        name,
    ))
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
        let rounds = file
            .rounds
            .0
            .into_iter()
            .map(|(name, round)| match resolved(round, substances) {
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

    /// A registry of one round whose fields are `fields`, each authored with a fixture provenance.
    fn one(fields: &[(&str, &str)]) -> Result<RoundRegistry, RoundError> {
        let body = fields
            .iter()
            .map(|(field, value)| format!(r#"{field}: ({value}, Tuned("fixture"))"#))
            .collect::<Vec<_>>()
            .join(", ");
        RoundRegistry::from_ron(
            &format!(r#"(rounds: {{ "Probe": ({body}) }})"#),
            SubstanceRegistry::shipped_static(),
        )
    }

    /// The fixture round's fields: fixture data, not a shipped round.
    const FIXTURE: [(&str, &str); 6] = [
        ("name", r#""Probe Mk. 1""#),
        ("core", r#""GunSteel""#),
        ("nose", "Ogive(crh: 1.0)"),
        ("mass", "5.0"),
        ("caliber", "0.05"),
        ("muzzle_speed", "700.0"),
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

    /// The fixture round loads, and its nose factor is the one DERIVED from its authored nose.
    #[test]
    fn the_fixture_round_derives_its_nose_factor_at_load() {
        let round = with("mass", "5.0").unwrap().get("Probe").unwrap();
        assert_eq!(
            round.body.nose_factor,
            NoseShape::Ogive { crh: 1.0 }.factor()
        );
        assert_eq!(round.body.mass, 5.0);
        assert_eq!(round.body.caliber, 0.05);
        assert_eq!(round.muzzle_speed, 700.0);
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
            muzzle_speed: (700.0, Tuned("fixture"))) })"#;
        let err = RoundRegistry::from_ron(bare, substances).unwrap_err();
        assert!(err.to_string().contains("provenance"), "{err}");

        let round = FIXTURE
            .iter()
            .map(|(field, value)| format!(r#"{field}: ({value}, Tuned("fixture"))"#))
            .collect::<Vec<_>>()
            .join(", ");
        let twice = format!(r#"(rounds: {{ "Probe": ({round}), "Probe": ({round}) }})"#);
        let err = RoundRegistry::from_ron(&twice, substances).unwrap_err();
        assert!(err.to_string().contains("twice"), "{err}");

        let stray = format!(r#"(rounds: {{ "Probe": ({round}, hardness: (3.0, Tuned("x"))) }})"#);
        assert!(RoundRegistry::from_ron(&stray, substances).is_err());
    }

    /// Each nose family derives its factor by its own formula (a hemisphere and a half-calibre
    /// cone are `½`, DERIVED; a flat face `1`), and a parameter outside the family's range is a
    /// load error naming the round.
    #[test]
    fn nose_shapes_derive_their_factor_and_reject_out_of_range_parameters() {
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
