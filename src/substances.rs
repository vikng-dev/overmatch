//! The global substance registry — the numeric half of the material library
//! (`assets/materials/materials.ron`).
//!
//! The Blender side is `assets/materials/materials.blend`: one material datablock per entry,
//! LINKED into each tank's `.blend` and assigned to its `*_Ballistic` meshes. The datablock NAME is
//! the join key; this module owns the parsed scalars. Design doc §12 ("no numbers in the model")
//! and §13.7 (substance is authored per plate, paint is a runtime livery).
//!
//! SEAM — NOT YET CONSUMED BY THE GAME. The bake still reads per-node `material_factor` out of
//! `<tank>.tank.ron` ([`crate::spec::VolumeSpec`]). Slice 3 of the §13 union-walk arc replaces that
//! with a lookup here, keyed by the substance name the glTF material carries on each ballistic
//! primitive. Until then this is the authored contract, parsed and asserted by tests so the file
//! cannot drift before the consumer arrives.
//!
//! FAIL-LOUD, NO DEFAULTS (ADR-0011, and §13.1's defect class): an unknown substance name is a hard
//! error naming the name, never a fallback factor. Silent-zero armour is exactly what §13 exists to
//! kill, and a defaulted material factor is the same defect wearing the registry's clothes.

use serde::Deserialize;
use std::collections::HashMap;

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
    substances: HashMap<String, Substance>,
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
    #[allow(
        dead_code,
        reason = "slice-3 seam: the bake binds to this when the walk lands"
    )]
    pub fn shipped() -> Self {
        Self::from_ron(MATERIALS_RON)
            .unwrap_or_else(|err| panic!("substances: embedded materials.ron is unusable: {err}"))
    }

    /// Look a substance up by material datablock name. Hard error on an unknown name — never a
    /// default (module doc).
    #[allow(
        dead_code,
        reason = "slice-3 seam: the bake binds to this when the walk lands"
    )]
    pub fn get(&self, name: &str) -> Result<Substance, SubstanceError> {
        self.substances.get(name).copied().ok_or_else(|| {
            // Sorted so the diagnostic is stable across runs — `HashMap` iteration order is not.
            let mut known: Vec<String> = self.substances.keys().cloned().collect();
            known.sort();
            SubstanceError::Unknown {
                name: name.to_string(),
                known,
            }
        })
    }

    /// The §13.2 field value for a named substance — the one number the union walk consumes.
    #[allow(
        dead_code,
        reason = "slice-3 seam: the bake binds to this when the walk lands"
    )]
    pub fn factor(&self, name: &str) -> Result<f32, SubstanceError> {
        self.get(name).map(|substance| substance.factor)
    }

    /// How many substances the registry carries.
    #[allow(
        dead_code,
        reason = "slice-3 seam: the bake binds to this when the walk lands"
    )]
    pub fn len(&self) -> usize {
        self.substances.len()
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
        assert_eq!(registry.len(), EXPECTED.len());
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
        assert_eq!(registry.factor("RHA").unwrap(), 1000.0);
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
        assert!(registry.factor("Unobtainium").is_err());
    }

    #[test]
    fn an_unknown_field_fails_the_parse() {
        let err = SubstanceRegistry::from_ron(
            r#"(substances: { "RHA": (factor: 1000.0, paintable: true, hardness: 3.0) })"#,
        )
        .expect_err("deny_unknown_fields must reject a stray key");
        assert!(matches!(err, SubstanceError::Parse(_)));
    }
}
