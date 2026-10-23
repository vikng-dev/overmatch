//! The sturdiness classes a box's HP pool is authored from, read from
//! `assets/ballistics/box_classes.ron`: HP points per m² of the box's mean face, the mean of the
//! three axis-aligned face areas of its bounds in the tank's sim geometry. A tank file authors
//! each pool as its class's rate times that face; the pool gate (`src/tests/box_pools.rs`) holds
//! every tank asset to it. The class follows the box's facets ([`BoxClass::of`]). Design doc §6.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the pool gate reads the classes under test; nothing reads them at runtime yet"
    )
)]

use serde::Deserialize;

use crate::provenance::Sourced;
use crate::spec::ComponentSpec;

/// The shipped classes, embedded so they are read while the app is built (ADR-0014: built from
/// data, synchronously).
const BOX_CLASSES_RON: &str = include_str!("../../assets/ballistics/box_classes.ron");

/// A box's sturdiness class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoxClass {
    /// A crew station's box.
    Crew,
    /// An ammunition box.
    Ammo,
    /// Every other box with an HP pool: a module.
    Hard,
}

impl BoxClass {
    /// The class `component`'s facets choose: the crew facet is [`Self::Crew`], the ammo facet
    /// [`Self::Ammo`], anything else with a pool [`Self::Hard`].
    pub(crate) fn of(component: &ComponentSpec) -> Self {
        if component.crew.is_some() {
            Self::Crew
        } else if component.ammo {
            Self::Ammo
        } else {
            Self::Hard
        }
    }
}

/// The file as authored: every value with its provenance ([`crate::provenance`]).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassFile {
    crew: Sourced<u32>,
    ammo: Sourced<u32>,
    hard: Sourced<u32>,
}

/// Each class's HP points per m² of mean face, validated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BoxClasses {
    crew: u32,
    ammo: u32,
    hard: u32,
}

impl BoxClasses {
    /// Parse and validate a class file, refusing a class of zero HP per m²: a box of that class
    /// would be authored with no pool.
    pub(crate) fn from_ron(text: &str) -> Result<Self, String> {
        let file: ClassFile = ron::de::from_str(text).map_err(|err| err.to_string())?;
        let classes = Self {
            crew: file.crew.0,
            ammo: file.ammo.0,
            hard: file.hard.0,
        };
        for (name, rate) in [
            ("crew", classes.crew),
            ("ammo", classes.ammo),
            ("hard", classes.hard),
        ] {
            if rate == 0 {
                return Err(format!("{name} must be at least 1 HP per m² (got 0)"));
            }
        }
        Ok(classes)
    }

    /// The shipped classes, parsed once. Panics on a malformed file: a broken authored contract is
    /// a build defect. [`super::sim_plugin`] calls this while the app is built.
    pub(crate) fn shipped() -> &'static Self {
        static CLASSES: std::sync::LazyLock<BoxClasses> = std::sync::LazyLock::new(|| {
            BoxClasses::from_ron(BOX_CLASSES_RON).unwrap_or_else(|err| {
                panic!("box classes: embedded box_classes.ron is unusable: {err}")
            })
        });
        &CLASSES
    }

    /// `class`'s HP points per m² of mean face.
    pub(crate) fn hp_per_square_metre(&self, class: BoxClass) -> u32 {
        match class {
            BoxClass::Crew => self.crew,
            BoxClass::Ammo => self.ammo,
            BoxClass::Hard => self.hard,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A class authored without its provenance, an unknown class or a class of zero is refused at
    /// parse, naming what is wrong.
    #[test]
    fn a_malformed_class_file_is_refused() {
        let sourced = r#"(crew: (150, Tuned("fixture")), ammo: (200, Tuned("fixture")),
            hard: (300, Tuned("fixture")))"#;
        let classes = BoxClasses::from_ron(sourced).expect("a sourced file parses");
        assert_eq!(classes.hp_per_square_metre(BoxClass::Crew), 150);
        assert_eq!(classes.hp_per_square_metre(BoxClass::Ammo), 200);
        assert_eq!(classes.hp_per_square_metre(BoxClass::Hard), 300);
        let bare = r#"(crew: 150, ammo: (200, Tuned("fixture")), hard: (300, Tuned("fixture")))"#;
        let error = BoxClasses::from_ron(bare).expect_err("a bare class is refused");
        assert!(error.contains("provenance"), "{error}");
        let unknown = r#"(crew: (150, Tuned("fixture")), ammo: (200, Tuned("fixture")),
            hard: (300, Tuned("fixture")), soft: (50, Tuned("fixture")))"#;
        assert!(BoxClasses::from_ron(unknown).is_err(), "an unknown class");
        let zero = r#"(crew: (150, Tuned("fixture")), ammo: (0, Tuned("fixture")),
            hard: (300, Tuned("fixture")))"#;
        let error = BoxClasses::from_ron(zero).expect_err("a zero class is refused");
        assert!(error.contains("ammo"), "{error}");
    }
}
