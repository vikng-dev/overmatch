//! The global substance registry — the numeric half of the material library
//! (`assets/materials/materials.ron`).
//!
//! The Blender side is `assets/materials/materials.blend`: one material datablock per entry,
//! LINKED into each tank's `.blend` and assigned to its ballistic meshes. The datablock NAME is
//! the join key; this module owns the parsed scalars. Design doc §12 ("no numbers in the model")
//! and §13.7 (substance is authored per plate, paint is a runtime livery).
//!
//! THIS IS THE CLASSIFIER, not just a table. [`crate::bake`] asks it about every glTF primitive's
//! material name: `Ok` means the primitive IS a ballistic volume made of that substance, `Err`
//! means it is ordinary art. Membership and resistance therefore come from one lookup and cannot
//! disagree — which is the whole point of retiring the `*_Ballistic` name marker. Per-node
//! `material_factor` in `<tank>.tank.ron` is gone.
//!
//! FAIL-LOUD, NO DEFAULTS (ADR-0011, and §13.1's defect class): an unknown substance name is a hard
//! error naming the name, never a fallback factor. Silent-zero armour is exactly what §13 exists to
//! kill, and a defaulted material factor is the same defect wearing the registry's clothes.

use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};
use std::collections::BTreeMap;

/// The shipped registry, embedded so the walk never depends on asset-server timing (the same reason
/// [`crate::bake`] embeds the Tiger spec).
const MATERIALS_RON: &str = include_str!("../assets/materials/materials.ron");

/// One substance's scalars. `factor` is the §13.2 field value: shell-resistance in reference-mm per
/// metre of chord, so a metre of `RHA` costs 1000 reference-mm and the union walk's
/// `∫ max(factor) dt` comes out in reference-mm directly.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Substance {
    /// Reference-mm of armour per metre of material.
    pub factor: f32,
    /// Whether the runtime livery composes onto this substance — exterior steels yes,
    /// interior/organic substances no. View-layer data; the walk never reads it.
    pub paintable: bool,
}

/// The parsed `materials.ron` — every substance in the game, keyed by material datablock name.
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct SubstanceRegistry {
    substances: SubstanceMap,
}

/// The substance table, with its validity enforced AT PARSE.
///
/// A plain `HashMap` cannot carry the contract: serde fills one by repeated insertion, so a
/// duplicated key silently overwrites and the file's second `"RHA"` wins with nobody told. Nor does
/// a derived map reject a factor that cannot be a field value. Both are the §13.1 defect class
/// dressed as data — a registry that answers `Ok` for a file it should have refused hands the walk
/// a wrong number instead of an error, and wrong armour is worse than absent armour because nothing
/// downstream can tell.
///
/// Validity is therefore checked where the entries arrive, not later at [`crate::ballistics`]'s
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
                    let factor = substance.factor;
                    if !factor.is_finite() {
                        return Err(de::Error::custom(format!(
                            "substance {name:?} has a non-finite factor ({factor}) — a factor is a \
                             field value the union walk takes `max` over and integrates; NaN \
                             poisons both the maximum and the sort order"
                        )));
                    }
                    if factor < 0.0 {
                        return Err(de::Error::custom(format!(
                            "substance {name:?} has a negative factor ({factor}) — negative \
                             resistance breaks §13.6 monotonicity: adding the volume would LOWER \
                             protection along the ray"
                        )));
                    }
                    // `-0.0` is canonicalized so "no resistance" has exactly one bit pattern; the
                    // walk coalesces canonical spans by factor BITS, and a second zero would cut a
                    // span where the field never changed.
                    let substance = Substance {
                        factor: if factor == 0.0 { 0.0 } else { factor },
                        ..substance
                    };
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

/// Why a registry could not be used. Both arms name the offending text so the failure is readable
/// without a debugger.
#[derive(Debug, Clone, PartialEq)]
pub enum SubstanceError {
    /// The RON did not parse.
    Parse(String),
    /// A lookup asked for a name the registry does not carry. Carries the name AND the registry's
    /// full vocabulary, because the usual cause is a Blender-side rename and the fix is to read off
    /// which spelling the file actually has.
    Unknown { name: String, known: Vec<String> },
}

impl std::fmt::Display for SubstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "materials.ron failed to parse: {err}"),
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
    /// Parse a registry from RON text.
    pub fn from_ron(text: &str) -> Result<Self, SubstanceError> {
        ron::de::from_str(text).map_err(|err| SubstanceError::Parse(err.to_string()))
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
        self.substances.0.keys().map(String::as_str).collect()
    }

    /// Look a substance up by material datablock name. Hard error on an unknown name — never a
    /// default (module doc).
    pub fn get(&self, name: &str) -> Result<Substance, SubstanceError> {
        self.substances.0.get(name).copied().ok_or_else(|| {
            SubstanceError::Unknown {
                name: name.to_string(),
                // Already ordered: the table is a `BTreeMap`, so the diagnostic is stable run to run.
                known: self.substances.0.keys().cloned().collect(),
            }
        })
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

    #[test]
    fn shipped_materials_ron_parses() {
        let registry = SubstanceRegistry::shipped();
        assert_eq!(registry.substances.0.len(), EXPECTED.len());
    }

    #[test]
    fn shipped_registry_carries_every_expected_substance() {
        let registry = SubstanceRegistry::shipped();
        for name in EXPECTED {
            let substance = registry
                .get(name)
                .unwrap_or_else(|err| panic!("{name} must be in the registry: {err}"));
            assert!(
                substance.factor > 0.0,
                "{name} must resist something (factor > 0)"
            );
        }
    }

    /// The RHA identity: reference-mm per metre is defined so that a metre of RHA costs 1000
    /// reference-mm. Every other factor is read against it, so it is pinned, not merely present.
    #[test]
    fn rha_is_the_thousand_reference_mm_per_metre_identity() {
        let registry = SubstanceRegistry::shipped();
        assert_eq!(registry.get("RHA").unwrap().factor, 1000.0);
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
        assert!(registry.get("Unobtainium").is_err());
    }

    #[test]
    fn an_unknown_field_fails_the_parse() {
        let err = SubstanceRegistry::from_ron(
            r#"(substances: { "RHA": (factor: 1000.0, paintable: true, hardness: 3.0) })"#,
        )
        .expect_err("deny_unknown_fields must reject a stray key");
        assert!(matches!(err, SubstanceError::Parse(_)));
    }

    /// A duplicated datablock name means two different answers for one join key. Serde's default
    /// map fill would silently keep the last one; the file must be refused instead, naming the key.
    #[test]
    fn a_duplicated_substance_name_fails_the_parse() {
        let err = SubstanceRegistry::from_ron(
            r#"(substances: {
                "RHA":  (factor: 1000.0, paintable: true),
                "Cast": (factor:  900.0, paintable: true),
                "RHA":  (factor:   10.0, paintable: false),
            })"#,
        )
        .expect_err("a duplicate key must not silently overwrite");
        assert!(err.to_string().contains("RHA"), "{err}");
        assert!(err.to_string().contains("twice"), "{err}");
    }

    /// A negative factor breaks §13.6 monotonicity — adding the volume would LOWER protection.
    #[test]
    fn a_negative_factor_fails_the_parse() {
        let err = SubstanceRegistry::from_ron(
            r#"(substances: { "Rubber": (factor: -50.0, paintable: false) })"#,
        )
        .expect_err("a negative factor must not parse");
        assert!(err.to_string().contains("Rubber"), "{err}");
        assert!(err.to_string().contains("negative"), "{err}");
    }

    /// A non-finite factor poisons both the union `max` and the total order the walk sorts on.
    #[test]
    fn a_non_finite_factor_fails_the_parse() {
        for literal in ["NaN", "inf", "-inf"] {
            let err = SubstanceRegistry::from_ron(&format!(
                r#"(substances: {{ "Flesh": (factor: {literal}, paintable: false) }})"#
            ))
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
        let registry = SubstanceRegistry::from_ron(
            r#"(substances: { "Void": (factor: -0.0, paintable: false) })"#,
        )
        .expect("zero resistance is legal, it is just nothing");
        assert_eq!(
            registry.get("Void").unwrap().factor.to_bits(),
            0.0f32.to_bits()
        );
    }
}
