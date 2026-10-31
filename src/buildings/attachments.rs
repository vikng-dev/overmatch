//! `assets/materials/attachments.ron`: the shared list of attachment kinds, how two parts of a
//! building hold together where they touch (the game's environment destruction, game state only).
//! A building names a kind for each joint between two of its components
//! ([`super::schema::Bond::Attached`]); the list says what that kind holds by, as the fracture
//! laws read it ([`Bond`]): the seam of a substance (`materials.ron` owns those numbers), a seam
//! the list states with its sources, or a rest. The file's comments carry each kind's meaning.
//!
//! Embedded in the build as `materials.ron` is, and parsed once ([`Attachments::shipped_static`]).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use super::fracture::{Bond, Masonry, Seam, Shear};
use crate::keyed_map::KeyedMap;
use crate::provenance::Sourced;
use crate::substances::SubstanceRegistry;

/// The list's path under the repository root: the file its refusals name.
pub const ATTACHMENTS_PATH: &str = "assets/materials/attachments.ron";

const ATTACHMENTS_RON: &str = include_str!("../../assets/materials/attachments.ron");

/// What fixes one kind, as the file states it (the file's header).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum Row {
    Seam(String),
    Stated {
        tensile_pa: Sourced<f64>,
        fracture_energy_j_per_m2: Sourced<f64>,
    },
    Resting,
}

/// The attachment kinds by name, each resolved to what the fracture laws hold its joints by.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachments {
    kinds: BTreeMap<String, Bond>,
}

impl Attachments {
    /// The shipped list, parsed once. Panics on a list that does not load: a broken authored
    /// contract is a build defect, as the embedded `materials.ron` is
    /// ([`SubstanceRegistry::shipped`]).
    pub fn shipped_static() -> &'static Self {
        static LIST: LazyLock<Attachments> = LazyLock::new(|| {
            Attachments::from_ron(ATTACHMENTS_RON, SubstanceRegistry::shipped_static())
                .unwrap_or_else(|err| panic!("attachments: {ATTACHMENTS_PATH}: {err}"))
        });
        &LIST
    }

    /// Parse a list's text and resolve each kind against `registry`. Refuses a text that does not
    /// parse (a kind missing one of its two numbers or its source, a name stated twice, a field
    /// the format does not hold), a stated number that is not finite and positive, and the seam
    /// of a substance that states none, each naming the kind, the field and the fix.
    fn from_ron(text: &str, registry: &SubstanceRegistry) -> Result<Self, String> {
        let KeyedMap(rows) = ron::from_str::<KeyedMap<Row>>(text).map_err(|err| {
            format!(
                "{err} — a kind is `Seam(\"<substance>\")`, `Stated(tensile_pa: (value, \
                 Kind(\"source\")), fracture_energy_j_per_m2: (value, Kind(\"source\")))` or \
                 `Resting`"
            )
        })?;
        let kinds = rows
            .into_iter()
            .map(|(name, row)| {
                let bond = match row {
                    Row::Seam(substance) => Bond::Bonded(
                        Masonry::of(registry, &substance)
                            .map_err(|err| format!("kind {name:?}: {err}"))?
                            .ok_or_else(|| {
                                format!(
                                    "kind {name:?} takes the seam of {substance:?}, whose \
                                     materials.ron row states no bulk facts: name a substance \
                                     that states a seam, or state the kind's two numbers"
                                )
                            })?
                            .seam,
                    ),
                    Row::Stated {
                        tensile_pa: Sourced(tensile),
                        fracture_energy_j_per_m2: Sourced(energy),
                    } => {
                        for (field, value) in [
                            ("tensile_pa", tensile),
                            ("fracture_energy_j_per_m2", energy),
                        ] {
                            if !(value.is_finite() && value > 0.0) {
                                return Err(format!(
                                    "kind {name:?}: {field} {value} holds nothing: state it \
                                     finite and positive"
                                ));
                            }
                        }
                        // The kind states its opening alone, so its seam answers by the opening
                        // law alone: no punch ring forms across it.
                        Bond::Bonded(Seam {
                            tensile: tensile as f32,
                            fracture_energy: energy as f32,
                            shear: Shear::OpeningOnly,
                        })
                    }
                    Row::Resting => Bond::Resting,
                };
                Ok((name, bond))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { kinds })
    }

    /// What kind `name` holds its joints by. Refuses a name the list does not hold, naming the
    /// kinds it does and the fix.
    pub(crate) fn bond(&self, name: &str) -> Result<Bond, String> {
        self.kinds.get(name).copied().ok_or_else(|| {
            format!(
                "kind {name:?} is not in {ATTACHMENTS_PATH}, which holds [{}]: name one of those, \
                 or add the kind there with its two numbers and their sources",
                self.kinds
                    .keys()
                    .map(|kind| format!("{kind:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }

    /// Whether the list holds kind `name`, refusing as [`Self::bond`] does: the bake's check of a
    /// spec, which never reads what a kind holds by.
    pub fn holds(&self, name: &str) -> Result<(), String> {
        self.bond(name).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> &'static SubstanceRegistry {
        SubstanceRegistry::shipped_static()
    }

    /// Every declared kind resolves to its own source, whatever names or number of kinds the
    /// list carries. Fracture reads these bonds: copying a seam or treating rest as zero-strength
    /// glue would substitute a different construction for the authored one.
    #[test]
    fn every_declared_kind_resolves_to_its_source() {
        let list = Attachments::shipped_static();
        let KeyedMap(rows) = ron::from_str::<KeyedMap<Row>>(ATTACHMENTS_RON).unwrap();
        assert_eq!(list.kinds.len(), rows.len());
        for (name, row) in rows {
            let expected = match row {
                Row::Seam(substance) => Bond::Bonded(
                    Masonry::of(registry(), &substance)
                        .unwrap()
                        .expect("a declared seam has bulk facts")
                        .seam,
                ),
                Row::Stated {
                    tensile_pa: Sourced(tensile),
                    fracture_energy_j_per_m2: Sourced(energy),
                } => Bond::Bonded(Seam {
                    tensile: tensile as f32,
                    fracture_energy: energy as f32,
                    shear: Shear::OpeningOnly,
                }),
                Row::Resting => Bond::Resting,
            };
            assert_eq!(list.bond(&name), Ok(expected), "{name}");
        }
    }

    /// An unknown kind is refused with the available kinds and fix. FIXTURE DATA: this small
    /// list makes absence independent of additions to the shipped attachment table.
    #[test]
    fn a_kind_the_list_does_not_hold_is_refused_naming_the_fix() {
        let list = Attachments::from_ron(r#"{"fixture-kind": Resting}"#, registry()).unwrap();
        let err = list.bond("unknown-kind").unwrap_err();
        for part in [
            "\"unknown-kind\"",
            ATTACHMENTS_PATH,
            "\"fixture-kind\"",
            "add the kind there",
        ] {
            assert!(err.contains(part), "{part}: {err}");
        }
        assert!(list.holds("unknown-kind").is_err());
    }

    /// A KIND WITHOUT BOTH NUMBERS, OR WITHOUT THEIR SOURCES, DOES NOT LOAD: each of the labelled fixture
    /// kind's two numbers dropped, its source dropped, a number of no strength, a name
    /// stated twice, a field the format does not hold, and a seam of a substance that states none
    /// are each refused, naming the field or the kind. A list that let a kind load with a number
    /// missing would hold its joints by a code default.
    #[test]
    fn a_kind_missing_a_number_or_its_source_is_refused() {
        let nailed = |fields: &str| format!("{{\"nailed\": Stated({fields})}}");
        let tensile = "tensile_pa: (3240.0, Derived(\"fixture\"))";
        let energy = "fracture_energy_j_per_m2: (65.0, Derived(\"fixture\"))";
        Attachments::from_ron(&nailed(&format!("{tensile}, {energy}")), registry())
            .expect("both numbers, each sourced");
        for (text, names) in [
            (nailed(tensile), "fracture_energy_j_per_m2"),
            (nailed(energy), "tensile_pa"),
            (
                nailed(&format!("tensile_pa: 3240.0, {energy}")),
                "provenance",
            ),
            (
                nailed(&format!(
                    "tensile_pa: (0.0, Derived(\"fixture\")), {energy}"
                )),
                "tensile_pa 0",
            ),
            (
                nailed(&format!("{tensile}, {energy}, spacing_m: 0.6")),
                "spacing_m",
            ),
            (
                "{\"nailed\": Resting, \"nailed\": Resting}".to_owned(),
                "twice",
            ),
            ("{\"mortared\": Seam(\"RHA\")}".to_owned(), "\"mortared\""),
            ("{\"mortared\": Seam(\"Adobe\")}".to_owned(), "Adobe"),
        ] {
            let err = Attachments::from_ron(&text, registry()).unwrap_err();
            assert!(err.contains(names), "{text}: {err}");
        }
    }
}
