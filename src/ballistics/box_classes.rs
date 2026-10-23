//! The HP pools of the boxes that carry one, read from `assets/ballistics/box_classes.ron`: the
//! one crew pool every crew station of every tank shares ([`BoxClasses::crew_hp`]), which the tank
//! file parse fills in and refuses to read from a tank file (`src/spec.rs`), and the sturdiness
//! classes every other pool is authored from, HP points per m² of the box's mean face (the mean of
//! the three axis-aligned face areas of its bounds in the tank's sim geometry). The pool gate
//! (`src/tests/box_pools.rs`) holds every tank asset to both. The class follows the box's facets
//! ([`BoxClass::of`]). Design doc §6.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the pool gate reads the classes under test; at runtime only the crew pool is read"
    )
)]

use serde::Deserialize;

use crate::provenance::Sourced;
use crate::spec::ComponentSpec;

/// The shipped classes, embedded so they are read while the app is built (ADR-0014: built from
/// data, synchronously).
const BOX_CLASSES_RON: &str = include_str!("../../assets/ballistics/box_classes.ron");

/// The sturdiness class of a box whose pool is authored from its face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoxClass {
    /// An ammunition box.
    Ammo,
    /// Every other box with an authored pool: a module.
    Hard,
}

impl BoxClass {
    /// The class `component`'s facets choose: the ammo facet is [`Self::Ammo`], anything else
    /// [`Self::Hard`]; `None` for a crew station, whose pool is the shared
    /// [`BoxClasses::crew_hp`] and has no class.
    pub(crate) fn of(component: &ComponentSpec) -> Option<Self> {
        if component.crew.is_some() {
            None
        } else if component.ammo {
            Some(Self::Ammo)
        } else {
            Some(Self::Hard)
        }
    }
}

/// The file as authored: every value with its provenance ([`crate::provenance`]).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassFile {
    crew_hp: Sourced<u32>,
    ammo: Sourced<u32>,
    hard: Sourced<u32>,
}

/// The shared crew pool and each class's HP points per m² of mean face, validated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BoxClasses {
    crew_hp: u32,
    ammo: u32,
    hard: u32,
}

impl BoxClasses {
    /// Parse and validate a class file, refusing a crew pool of zero or a class of zero HP per m²:
    /// a box of either would have no pool.
    pub(crate) fn from_ron(text: &str) -> Result<Self, String> {
        let file: ClassFile = ron::de::from_str(text).map_err(|err| err.to_string())?;
        let classes = Self {
            crew_hp: file.crew_hp.0,
            ammo: file.ammo.0,
            hard: file.hard.0,
        };
        for (name, rate) in [
            ("crew_hp", classes.crew_hp),
            ("ammo", classes.ammo),
            ("hard", classes.hard),
        ] {
            if rate == 0 {
                return Err(format!("{name} must be at least 1 (got 0)"));
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

    /// The HP pool of every crew station of every tank, in whole HP points.
    pub(crate) fn crew_hp(&self) -> u32 {
        self.crew_hp
    }

    /// `class`'s HP points per m² of mean face.
    pub(crate) fn hp_per_square_metre(&self, class: BoxClass) -> u32 {
        match class {
            BoxClass::Ammo => self.ammo,
            BoxClass::Hard => self.hard,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value authored without its provenance, an unknown entry or a zero is refused at parse,
    /// naming what is wrong.
    #[test]
    fn a_malformed_class_file_is_refused() {
        let sourced = r#"(crew_hp: (30, Tuned("fixture")), ammo: (200, Tuned("fixture")),
            hard: (300, Tuned("fixture")))"#;
        let classes = BoxClasses::from_ron(sourced).expect("a sourced file parses");
        assert_eq!(classes.crew_hp(), 30);
        assert_eq!(classes.hp_per_square_metre(BoxClass::Ammo), 200);
        assert_eq!(classes.hp_per_square_metre(BoxClass::Hard), 300);
        let bare = r#"(crew_hp: 30, ammo: (200, Tuned("fixture")), hard: (300, Tuned("fixture")))"#;
        let error = BoxClasses::from_ron(bare).expect_err("a bare value is refused");
        assert!(error.contains("provenance"), "{error}");
        let unknown = r#"(crew_hp: (30, Tuned("fixture")), ammo: (200, Tuned("fixture")),
            hard: (300, Tuned("fixture")), crew: (150, Tuned("fixture")))"#;
        assert!(BoxClasses::from_ron(unknown).is_err(), "an unknown entry");
        for (zero, name) in [
            (
                r#"(crew_hp: (0, Tuned("fixture")), ammo: (200, Tuned("fixture")),
                    hard: (300, Tuned("fixture")))"#,
                "crew_hp",
            ),
            (
                r#"(crew_hp: (30, Tuned("fixture")), ammo: (0, Tuned("fixture")),
                    hard: (300, Tuned("fixture")))"#,
                "ammo",
            ),
        ] {
            let error = BoxClasses::from_ron(zero).expect_err("a zero is refused");
            assert!(error.contains(name), "{error}");
        }
    }
}
