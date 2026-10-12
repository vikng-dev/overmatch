//! The global substance registry — the numeric half of the material library
//! (`assets/materials/materials.ron`).
//!
//! The Blender side is `assets/materials/materials.blend`: one material datablock per entry,
//! LINKED into each tank's `.blend` and assigned to its ballistic meshes. The datablock NAME is
//! the join key; this module owns the parsed scalars. Design doc §12 ("no numbers in the model")
//! and §13.7 (substance is authored per plate, paint is a runtime livery). The target structure
//! (material facts, declared effective behaviour, mechanisms reading them) is
//! `.agents/scratch/damage-cam-2026-10-08/research/recommendation-solid-shot.md` Part 1.
//!
//! THIS IS THE CLASSIFIER, not just a table. [`crate::bake`] asks it about every glTF primitive's
//! material name: `Ok` means the primitive IS a ballistic volume made of that substance, `Err`
//! means it is ordinary art. Membership and resistance therefore come from one lookup and cannot
//! disagree.
//!
//! # What an entry states
//!
//! - [`MaterialFacts`]: density, flow strength, Young's modulus and Poisson's ratio, for a substance
//!   that is a structural metal with a sourced row. `None` where no fact is known — never a guess.
//! - [`Resistance`]: how a round's force law is resisted. Exactly one entry is the
//!   [`Resistance::Reference`] — the reference steel, whose facts ARE the force law's reference
//!   constants and whose millimetre is the reference-mm. Every other entry DECLARES its effective
//!   behaviour ([`Resistance::Effective`]): it resists as a fixed fraction of the reference force at
//!   every speed. Declared, never inferred from a fill fraction or from facts the law does not yet
//!   read.
//!
//! FAIL-LOUD, NO DEFAULTS (ADR-0011, and §13.1's defect class): an unknown substance name is a hard
//! error naming the name, never a fallback; a malformed entry is refused at parse, naming the entry.

use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};
use std::collections::BTreeMap;

/// The shipped registry, embedded so the walk never depends on asset-server timing (the same reason
/// [`crate::bake`] embeds the Tiger spec).
const MATERIALS_RON: &str = include_str!("../assets/materials/materials.ron");

/// Reference-mm per metre of the reference substance: the unit's definition (a millimetre of the
/// reference steel is one reference-mm), not a measured number.
pub const REFERENCE_FACTOR: f32 = 1000.0;

/// The facts a structural metal is stated by. SI units throughout.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialFacts {
    /// kg/m³.
    pub density: f64,
    /// Representative flow stress at large strain (Pa) — the cavity-expansion law's `Y`.
    pub flow_strength: f64,
    /// Pa.
    pub youngs_modulus: f64,
    /// Dimensionless, in `(0, ½)`.
    pub poisson_ratio: f64,
}

impl MaterialFacts {
    /// Cavity-expansion strength term `a = A_CE · Y` (Pa), in the compressible
    /// elastic–perfectly-plastic form `A_CE = (2/3) · (1 + ln(E / (3 (1 − ν) Y)))`. At `ν = ½` it is
    /// the incompressible form `(2/3) · (1 + ln(2E / (3Y)))`.
    ///
    /// A DERIVED prior from material facts, not validated against thick-steel depth data; the
    /// compressible form was read in restatement, not in its primary (Hill 1950; Forrestal & Luk
    /// 1988).
    pub fn cavity_strength(&self) -> f64 {
        let ratio = self.youngs_modulus / (3.0 * (1.0 - self.poisson_ratio) * self.flow_strength);
        let a_ce = 2.0 / 3.0 * (1.0 + ratio.ln());
        a_ce * self.flow_strength
    }

    /// Every fact finite and in its physical domain, and the strength term they give finite and
    /// positive. Names the first violation.
    fn validate(&self) -> Result<(), String> {
        for (field, value) in [
            ("density", self.density),
            ("flow_strength", self.flow_strength),
            ("youngs_modulus", self.youngs_modulus),
            ("poisson_ratio", self.poisson_ratio),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("{field} must be finite and positive (got {value})"));
            }
        }
        if self.poisson_ratio >= 0.5 {
            return Err(format!(
                "poisson_ratio must be below ½ (got {}) — the cavity law divides by 1 − ν and a \
                 solid with ν ≥ ½ has no finite bulk modulus",
                self.poisson_ratio
            ));
        }
        let strength = self.cavity_strength();
        if !strength.is_finite() || strength <= 0.0 {
            return Err(format!(
                "the facts give a cavity strength of {strength} Pa — E / (3 (1 − ν) Y) is too small \
                 for the elastic–plastic cavity law to state a resistance"
            ));
        }
        Ok(())
    }
}

/// How a substance resists a round's force law.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum Resistance {
    /// The reference steel: its facts are the force law's reference constants, and one metre of it
    /// is [`REFERENCE_FACTOR`] reference-mm by definition.
    Reference,
    /// A declared effective behaviour: reference-mm per metre of chord, i.e. the substance resists
    /// as `factor / REFERENCE_FACTOR` of the reference steel's force at every speed. The value is
    /// authored, not derived.
    Effective(f32),
}

/// A substance's identity: its position in the registry's name order. Stable for one registry;
/// never a wire or save identity (the registry's own text is).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SubstanceId(u64);

impl SubstanceId {
    /// A fixture substance for tests that build volumes without a registry: one id per factor, so
    /// fixture plates of one factor are one substance. Disjoint from every registry id.
    #[cfg(test)]
    pub(crate) fn fixture(factor: f32) -> Self {
        Self(1 << 32 | factor.to_bits() as u64)
    }
}

/// One substance's entry.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Substance {
    pub resistance: Resistance,
    /// Whether the runtime livery composes onto this substance — exterior steels yes,
    /// interior/organic substances no. View-layer data; the walk never reads it.
    pub paintable: bool,
    /// The substance's material facts, or `None` where none are sourced.
    pub facts: Option<MaterialFacts>,
}

impl Substance {
    /// Reference-mm per metre of chord: [`REFERENCE_FACTOR`] for the reference, the declared value
    /// otherwise.
    pub fn factor(&self) -> f32 {
        match self.resistance {
            Resistance::Reference => REFERENCE_FACTOR,
            Resistance::Effective(factor) => factor,
        }
    }
}

/// The parsed `materials.ron` — every substance in the game, keyed by material datablock name.
#[derive(Clone, Debug)]
pub struct SubstanceRegistry {
    substances: BTreeMap<String, Substance>,
    /// The one [`Resistance::Reference`] entry's name.
    reference: String,
}

/// The file's shape before the whole-registry checks ([`SubstanceRegistry::from_ron`]).
#[derive(Deserialize)]
#[serde(rename = "SubstanceRegistry", deny_unknown_fields)]
struct RegistryFile {
    substances: SubstanceMap,
}

/// The substance table, with each entry's validity enforced AT PARSE.
///
/// A plain `HashMap` cannot carry the contract: serde fills one by repeated insertion, so a
/// duplicated key silently overwrites and the file's second `"RHA"` wins with nobody told. Nor does
/// a derived map reject a value that cannot be a resistance. Both are the §13.1 defect class
/// dressed as data — a registry that answers `Ok` for a file it should have refused hands the walk
/// a wrong number instead of an error.
///
/// Validity is checked where the entries arrive, not later at [`crate::ballistics`]'s
/// `VolumeTable`: that one guards the WALK, this one guards the FILE, and a bad file must fail at
/// the bake rather than at the first shot that happens to cross the offending plate.
#[derive(Clone, Debug, Default)]
struct SubstanceMap(BTreeMap<String, Substance>);

impl<'de> Deserialize<'de> for SubstanceMap {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor;

        impl<'de> Visitor<'de> for MapVisitor {
            type Value = SubstanceMap;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map of material datablock name to substance")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                // BTreeMap, not HashMap: the diagnostic vocabulary and any future iteration are then
                // ordered by construction rather than by hash seed.
                let mut out: BTreeMap<String, Substance> = BTreeMap::new();
                while let Some((name, substance)) = access.next_entry::<String, Substance>()? {
                    let substance = checked(&name, substance).map_err(de::Error::custom)?;
                    if out.insert(name.clone(), substance).is_some() {
                        return Err(de::Error::custom(format!(
                            "substance {name:?} is declared twice — the datablock name is the join \
                             key between materials.blend and this file, so a duplicate means two \
                             different answers for one key and the file cannot say which is meant"
                        )));
                    }
                }
                Ok(SubstanceMap(out))
            }
        }

        deserializer.deserialize_map(MapVisitor)
    }
}

/// One entry's own validity, with `-0.0` canonicalized.
fn checked(name: &str, substance: Substance) -> Result<Substance, String> {
    if let Some(facts) = &substance.facts {
        facts.validate().map_err(|reason| {
            format!("substance {name:?} has unusable material facts: {reason}")
        })?;
    }
    let resistance = match substance.resistance {
        Resistance::Reference => {
            if substance.facts.is_none() {
                return Err(format!(
                    "substance {name:?} is the reference but states no material facts — the \
                     reference's facts ARE the force law's reference constants"
                ));
            }
            Resistance::Reference
        }
        Resistance::Effective(factor) => {
            if !factor.is_finite() {
                return Err(format!(
                    "substance {name:?} has a non-finite effective factor ({factor}) — a factor \
                     is integrated along the chord, and NaN poisons the sum and the sort order"
                ));
            }
            if factor < 0.0 {
                return Err(format!(
                    "substance {name:?} has a negative effective factor ({factor}) — negative \
                     resistance breaks §13.6 monotonicity: adding the volume would LOWER \
                     protection along the ray"
                ));
            }
            // `-0.0` is canonicalized so "no resistance" has exactly one bit pattern; the walk
            // coalesces canonical spans by factor BITS, and a second zero would cut a span where
            // the field never changed.
            Resistance::Effective(if factor == 0.0 { 0.0 } else { factor })
        }
    };
    Ok(Substance {
        resistance,
        ..substance
    })
}

/// Why a registry could not be used. Every arm names the offending text so the failure is readable
/// without a debugger.
#[derive(Debug, Clone, PartialEq)]
pub enum SubstanceError {
    /// The RON did not parse, or an entry was refused.
    Parse(String),
    /// The entries parsed, but the registry does not name exactly one reference.
    Reference(String),
    /// A lookup asked for a name the registry does not carry. Carries the name AND the registry's
    /// full vocabulary, because the usual cause is a Blender-side rename and the fix is to read off
    /// which spelling the file actually has.
    Unknown { name: String, known: Vec<String> },
}

impl std::fmt::Display for SubstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "materials.ron failed to parse: {err}"),
            Self::Reference(err) => write!(f, "materials.ron names no usable reference: {err}"),
            Self::Unknown { name, known } => write!(
                f,
                "unknown substance {name:?} — assets/materials/materials.ron declares [{}]. \
                 A ballistic mesh is assigned a material datablock that the registry does not \
                 name; either the .blend link drifted or the entry was never authored.",
                known.join(", ")
            ),
        }
    }
}

impl SubstanceRegistry {
    /// Parse a registry from RON text: every entry valid, and exactly one reference.
    pub fn from_ron(text: &str) -> Result<Self, SubstanceError> {
        let file: RegistryFile =
            ron::de::from_str(text).map_err(|err| SubstanceError::Parse(err.to_string()))?;
        let substances = file.substances.0;
        let references: Vec<&String> = substances
            .iter()
            .filter(|(_, substance)| substance.resistance == Resistance::Reference)
            .map(|(name, _)| name)
            .collect();
        let reference = match references.as_slice() {
            [one] => (*one).clone(),
            [] => {
                return Err(SubstanceError::Reference(
                    "no substance is `resistance: Reference` — the force law has no reference \
                     constants and the reference-mm no definition"
                        .into(),
                ));
            }
            several => {
                return Err(SubstanceError::Reference(format!(
                    "{several:?} are all `resistance: Reference` — the reference-mm is a \
                     millimetre of ONE substance"
                )));
            }
        };
        Ok(Self {
            substances,
            reference,
        })
    }

    /// The shipped registry, parsed once.
    pub fn shipped_static() -> &'static Self {
        static REGISTRY: std::sync::LazyLock<SubstanceRegistry> =
            std::sync::LazyLock::new(SubstanceRegistry::shipped);
        &REGISTRY
    }

    /// The shipped registry. Panics on a malformed file, matching [`crate::bake`]'s treatment of the
    /// embedded tank spec: a broken authored contract is a build defect, not a runtime condition to
    /// degrade through.
    pub fn shipped() -> Self {
        Self::from_ron(MATERIALS_RON)
            .unwrap_or_else(|err| panic!("substances: embedded materials.ron is unusable: {err}"))
    }

    /// Every material datablock name the registry declares, in name order. The join key list
    /// itself: `materials.blend` carries exactly these datablocks, and the source lint holds the
    /// tank's materials to them through the canon file.
    pub fn keys(&self) -> Vec<&str> {
        self.substances.keys().map(String::as_str).collect()
    }

    /// Look a substance up by material datablock name. Hard error on an unknown name — never a
    /// default (module doc).
    pub fn get(&self, name: &str) -> Result<Substance, SubstanceError> {
        self.substances.get(name).copied().ok_or_else(|| {
            SubstanceError::Unknown {
                name: name.to_string(),
                // Already ordered: the table is a `BTreeMap`, so the diagnostic is stable run to run.
                known: self.substances.keys().cloned().collect(),
            }
        })
    }

    /// The identity of the substance named `name` in this registry — hard error on an unknown name.
    pub fn id(&self, name: &str) -> Result<SubstanceId, SubstanceError> {
        self.get(name)?;
        let index = self
            .substances
            .keys()
            .position(|key| key == name)
            .expect("present, checked above");
        Ok(SubstanceId(index as u64))
    }

    /// The reference steel's material facts — the force law's reference constants.
    pub fn reference_facts(&self) -> MaterialFacts {
        self.substances[&self.reference]
            .facts
            .expect("the reference states its facts, checked at parse")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every substance the shipped library declares. A rename or a deletion has to come here first,
    /// which is the point: the Blender link key and the code's vocabulary are one contract.
    const EXPECTED: [&str; 8] = [
        "RHA",
        "GunSteel",
        "Cast",
        "MildSteel",
        "EngineBlock",
        "Ammunition",
        "Rubber",
        "Flesh",
    ];

    /// A minimal valid registry around one extra entry, for the refusal tests.
    fn with(extra: &str) -> String {
        format!(
            r#"(substances: {{
                "Ref": (resistance: Reference, paintable: true, facts: Some((density: 7850.0,
                        flow_strength: 1.0e9, youngs_modulus: 205.0e9, poisson_ratio: 0.29))),
                {extra}
            }})"#
        )
    }

    #[test]
    fn shipped_materials_ron_parses() {
        let registry = SubstanceRegistry::shipped();
        assert_eq!(registry.substances.len(), EXPECTED.len());
    }

    #[test]
    fn shipped_registry_carries_every_expected_substance() {
        let registry = SubstanceRegistry::shipped();
        for name in EXPECTED {
            let substance = registry
                .get(name)
                .unwrap_or_else(|err| panic!("{name} must be in the registry: {err}"));
            assert!(
                substance.factor() > 0.0,
                "{name} must resist something (factor > 0)"
            );
        }
    }

    /// The reference is the steel the reference-mm is a millimetre of, and its facts are the ones
    /// the force law was stated with (fixture data, from the research table the shipped file
    /// cites).
    #[test]
    fn the_shipped_reference_is_rha_at_the_thousand_reference_mm_identity() {
        let registry = SubstanceRegistry::shipped();
        let rha = registry.get("RHA").unwrap();
        assert_eq!(rha.resistance, Resistance::Reference);
        assert_eq!(rha.factor(), REFERENCE_FACTOR);
        assert_eq!(
            registry.reference_facts(),
            MaterialFacts {
                density: 7850.0,
                flow_strength: 1.0e9,
                youngs_modulus: 205.0e9,
                poisson_ratio: 0.29,
            }
        );
    }

    #[test]
    fn unknown_name_is_an_error_naming_the_name_never_a_default() {
        let registry = SubstanceRegistry::shipped();
        let err = registry
            .get("Unobtainium")
            .expect_err("an unknown substance must not resolve");
        match &err {
            SubstanceError::Unknown { name, known } => {
                assert_eq!(name, "Unobtainium");
                assert!(known.contains(&"RHA".to_string()));
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
        assert!(err.to_string().contains("Unobtainium"));
    }

    #[test]
    fn an_unknown_field_fails_the_parse() {
        let err = SubstanceRegistry::from_ron(&with(
            r#""Steel": (resistance: Effective(900.0), paintable: true, facts: None, hardness: 3.0),"#,
        ))
        .expect_err("deny_unknown_fields must reject a stray key");
        assert!(matches!(err, SubstanceError::Parse(_)));
    }

    /// A duplicated datablock name means two different answers for one join key. Serde's default
    /// map fill would silently keep the last one; the file must be refused instead, naming the key.
    #[test]
    fn a_duplicated_substance_name_fails_the_parse() {
        let err = SubstanceRegistry::from_ron(&with(
            r#""Cast": (resistance: Effective(900.0), paintable: true, facts: None),
               "Cast": (resistance: Effective(10.0), paintable: false, facts: None),"#,
        ))
        .expect_err("a duplicate key must not silently overwrite");
        assert!(err.to_string().contains("Cast"), "{err}");
        assert!(err.to_string().contains("twice"), "{err}");
    }

    /// A negative factor breaks §13.6 monotonicity — adding the volume would LOWER protection.
    #[test]
    fn a_negative_factor_fails_the_parse() {
        let err = SubstanceRegistry::from_ron(&with(
            r#""Rubber": (resistance: Effective(-50.0), paintable: false, facts: None),"#,
        ))
        .expect_err("a negative factor must not parse");
        assert!(err.to_string().contains("Rubber"), "{err}");
        assert!(err.to_string().contains("negative"), "{err}");
    }

    /// A non-finite factor poisons the chord integral and the total order the walk sorts on.
    #[test]
    fn a_non_finite_factor_fails_the_parse() {
        for literal in ["NaN", "inf", "-inf"] {
            let err = SubstanceRegistry::from_ron(&with(&format!(
                r#""Flesh": (resistance: Effective({literal}), paintable: false, facts: None),"#
            )))
            .unwrap_err();
            assert!(
                err.to_string().contains("Flesh"),
                "factor {literal} must be refused by name, got {err}"
            );
        }
    }

    /// `-0.0` is canonicalized: "no resistance" gets exactly one bit pattern, because the walk
    /// coalesces its canonical spans by factor BITS and a second zero would cut a span where the
    /// field never changed.
    #[test]
    fn negative_zero_is_canonicalized() {
        let registry = SubstanceRegistry::from_ron(&with(
            r#""Void": (resistance: Effective(-0.0), paintable: false, facts: None),"#,
        ))
        .expect("zero resistance is legal, it is just nothing");
        assert_eq!(
            registry.get("Void").unwrap().factor().to_bits(),
            0.0f32.to_bits()
        );
    }

    /// Every fact out of its domain is refused by entry and by field.
    #[test]
    fn facts_out_of_domain_fail_the_parse() {
        let base = [
            ("density", "7850.0"),
            ("flow_strength", "1.0e9"),
            ("youngs_modulus", "205.0e9"),
            ("poisson_ratio", "0.29"),
        ];
        for (field, bad) in [
            ("density", "0.0"),
            ("density", "NaN"),
            ("flow_strength", "-1.0e9"),
            ("youngs_modulus", "inf"),
            ("poisson_ratio", "0.5"),
            ("poisson_ratio", "0.0"),
            // E / (3 (1 − ν) Y) far below e⁻¹: the cavity law states no positive strength.
            ("youngs_modulus", "1.0e6"),
        ] {
            let facts = base
                .iter()
                .map(|(name, value)| {
                    format!("{name}: {}", if *name == field { bad } else { value })
                })
                .collect::<Vec<_>>()
                .join(", ");
            let err = SubstanceRegistry::from_ron(&with(&format!(
                r#""Odd": (resistance: Effective(500.0), paintable: true, facts: Some(({facts}))),"#
            )))
            .expect_err("facts out of domain must not parse");
            assert!(
                err.to_string().contains("Odd"),
                "{field} = {bad} must be refused by name, got {err}"
            );
        }
    }

    /// The reference states the force law's constants, so it must carry facts; and there is
    /// exactly one.
    #[test]
    fn the_reference_is_exactly_one_entry_with_facts() {
        let factless = SubstanceRegistry::from_ron(
            r#"(substances: { "Ref": (resistance: Reference, paintable: true, facts: None) })"#,
        )
        .expect_err("a reference without facts must not parse");
        assert!(factless.to_string().contains("Ref"), "{factless}");

        let none = SubstanceRegistry::from_ron(
            r#"(substances: { "Cast": (resistance: Effective(900.0), paintable: true, facts: None) })"#,
        )
        .expect_err("a registry without a reference must not load");
        assert!(matches!(none, SubstanceError::Reference(_)), "{none}");

        let two = SubstanceRegistry::from_ron(&with(
            r#""Other": (resistance: Reference, paintable: true, facts: Some((density: 7800.0,
                flow_strength: 1.18e9, youngs_modulus: 190.0e9, poisson_ratio: 0.29))),"#,
        ))
        .expect_err("two references must not load");
        assert!(matches!(two, SubstanceError::Reference(_)), "{two}");
    }

    /// At `ν = ½` the compressible cavity form reduces to the incompressible one,
    /// `(2/3) · (1 + ln(2E / (3Y))) · Y`, for any material.
    #[test]
    fn cavity_strength_reduces_to_the_incompressible_form() {
        for (flow_strength, youngs_modulus) in [
            (1.0e9f64, 205.0e9f64),
            (0.276e9, 69.0e9),
            (0.4e9, 190.0e9),
            (2.7e9, 600.0e9),
        ] {
            let incompressible = 2.0 / 3.0
                * (1.0 + (2.0 * youngs_modulus / (3.0 * flow_strength)).ln())
                * flow_strength;
            let got = MaterialFacts {
                density: 1.0,
                flow_strength,
                youngs_modulus,
                poisson_ratio: 0.5,
            }
            .cavity_strength();
            assert!(
                (got / incompressible - 1.0).abs() < 1.0e-12,
                "Y {flow_strength}, E {youngs_modulus}: {got} vs {incompressible}"
            );
        }
    }

    /// The reference steel's facts give the DERIVED strength term
    /// `(2/3) · (1 + ln(205e9 / (3 · 0.71 · 1e9))) · 1e9 ≈ 3.711_258_7e9` Pa, to f32 precision.
    #[test]
    fn the_reference_cavity_strength_is_the_derived_value() {
        let a = SubstanceRegistry::shipped()
            .reference_facts()
            .cavity_strength();
        assert!(
            (a / 3.711_258_7e9 - 1.0).abs() < f32::EPSILON as f64,
            "strength term {a}"
        );
    }
}
