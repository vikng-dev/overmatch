//! The one HP pool every crew station of every tank shares ([`BoxClasses::crew_hp`]), read from
//! `assets/ballistics/box_classes.ron`: the tank file parse fills it in and refuses to read it from
//! a tank file (`src/spec.rs`). Every other pool is the plain `hp` its tank file authors, and
//! nothing here derives one. Design doc §6.
//!
//! The pen cam's potency bands part at this pool over a crew station's face
//! ([`BoxClasses::potency_thresholds`], design doc §5, "Potency colour"). The face is no datum of
//! this file: it is the struck tank's crew box, measured from its sim geometry by the caller
//! (`damage_cam::shapes::TankShapes::crew_face`), so the box's size has one source.

use serde::Deserialize;

use crate::provenance::Sourced;

/// The shipped file, embedded so it is read while the app is built (ADR-0014: built from data,
/// synchronously).
const BOX_CLASSES_RON: &str = include_str!("../../assets/ballistics/box_classes.ron");

/// The file as authored: every value with its provenance ([`crate::provenance`]).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassFile {
    crew_hp: Sourced<u32>,
}

/// The shared crew pool, validated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BoxClasses {
    crew_hp: u32,
}

/// The pen cam's potency band thresholds, HP points per m² of a crew station's face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PotencyThresholds {
    /// The least counted share of one HP point ([`crate::ballistics::least_counted_hp`]) over the
    /// crew face: a crew station's face wholly below it takes a total that converts to none.
    pub(crate) floor: f32,
    /// The crew pool over the crew face: a crew station's face wholly at or above it is knocked
    /// out.
    pub(crate) mid: f32,
}

impl BoxClasses {
    /// Parse and validate a class file, refusing a crew pool of zero (a station with no pool).
    pub(crate) fn from_ron(text: &str) -> Result<Self, String> {
        let file: ClassFile = ron::de::from_str(text).map_err(|err| err.to_string())?;
        let classes = Self {
            crew_hp: file.crew_hp.0,
        };
        if classes.crew_hp == 0 {
            return Err("crew_hp must be at least 1 (got 0)".to_owned());
        }
        Ok(classes)
    }

    /// The shipped file, parsed once. Panics on a malformed file: a broken authored contract is a
    /// build defect. [`super::sim_plugin`] calls this while the app is built.
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

    /// The thresholds the pen cam's potency bands part at for crew stations of mean face
    /// `crew_face` (m², positive and finite): the crew pool and the least counted share of an HP
    /// point, each over it.
    pub(crate) fn potency_thresholds(&self, crew_face: f32) -> PotencyThresholds {
        PotencyThresholds {
            floor: crate::ballistics::least_counted_hp() as f32 / crew_face,
            mid: self.crew_hp as f32 / crew_face,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture file: a crew pool of 30 HP, then `rest`.
    fn file(rest: &str) -> String {
        format!(r#"(crew_hp: (30, Tuned("fixture")){rest})"#)
    }

    /// A value authored without its provenance, an unknown entry (a retired per-m² class or crew
    /// face among them) or a zero crew pool is refused at parse, naming what is wrong. A pool or a
    /// face read from this file again, or a station left with no pool, fails here.
    #[test]
    fn a_malformed_class_file_is_refused() {
        let classes = BoxClasses::from_ron(&file("")).expect("a sourced file parses");
        assert_eq!(classes.crew_hp(), 30);
        let error = BoxClasses::from_ron("(crew_hp: 30)").expect_err("a bare value is refused");
        assert!(error.contains("provenance"), "{error}");
        for retired in [
            r#", crew_face_m2: (0.2, Tuned("fixture"))"#,
            r#", ammo: (150, Tuned("fixture"))"#,
            r#", hard: (300, Tuned("fixture"))"#,
        ] {
            assert!(BoxClasses::from_ron(&file(retired)).is_err(), "{retired}");
        }
        let error = BoxClasses::from_ron(&file("").replace("(30", "(0"))
            .expect_err("a zero pool is refused");
        assert!(error.contains("crew_hp"), "{error}");
    }

    /// The potency thresholds are the crew pool and the sim's rounding edge over the face asked
    /// for (fixture data: 30 HP over 0.25 m² is 120 HP per m², half a point over it 2). The pen
    /// cam's band promise relies on both; a threshold over any other face, or a floor read as a
    /// whole HP point, fails here.
    #[test]
    fn the_potency_thresholds_are_the_crew_pool_over_the_face() {
        let classes = BoxClasses::from_ron(&file("")).unwrap();
        assert_eq!(
            classes.potency_thresholds(0.25),
            PotencyThresholds {
                floor: 2.0,
                mid: 120.0
            }
        );
    }
}
