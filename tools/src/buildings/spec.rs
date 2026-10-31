//! `tools/buildings/<id>.house.ron`: the house generator's input. Read by the bake only; the game
//! never sees it.

use std::collections::BTreeMap;

use overmatch::buildings::schema::ElementKind;
use serde::Deserialize;

use super::house::Material;

/// One house (the file's own comments carry each field's meaning and units).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct House {
    pub id: String,
    pub width_m: f64,
    pub length_m: f64,
    pub wall_thickness_m: f64,
    pub course_m: f64,
    pub module_m: f64,
    pub foundation_courses: u32,
    pub eaves_course: u32,
    /// Rise over run, integers so every slope is an exact ratio.
    pub pitch: (u32, u32),
    pub plate_height_m: f64,
    pub board_width_m: f64,
    pub board_thickness_m: f64,
    pub boards_per_slope: u32,
    pub gable_overhang_m: f64,
    pub walls: Walls,
    /// What holds the parts of two components where they touch, by their elements' kinds.
    pub joins: Vec<Join>,
    /// The fixings a join may name, by name.
    pub fixings: BTreeMap<String, Fixing>,
    pub cuts: Cuts,
}

/// What holds every joint between an element of one of `kinds` and an element of the other (in
/// either order) where the two lie in different components.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Join {
    pub kinds: (ElementKind, ElementKind),
    pub by: Hold,
}

impl Join {
    /// Whether this join holds the pair of kinds `(a, b)`, in either order.
    pub fn joins(&self, (a, b): (ElementKind, ElementKind)) -> bool {
        self.kinds == (a, b) || self.kinds == (b, a)
    }
}

/// How one part is held to another it touches.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub enum Hold {
    /// By a fixing of [`House::fixings`], named.
    Fixed(String),
    /// Laid on it, nothing fixing it: held by weight and friction alone.
    Resting,
}

/// A fixing between two components (the spec's comments carry each value's source).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub enum Fixing {
    /// The seam of a material: a masonry's own mortar.
    Seam(Material),
    /// A seam stated here: its tensile strength across the joint's area, Pa, and its mode-I
    /// fracture energy, J/m².
    Stated {
        tensile_pa: f64,
        fracture_energy_j_per_m2: f64,
    },
}

/// The openings of each wall.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Walls {
    pub east: Vec<Opening>,
    pub west: Vec<Opening>,
    pub north: Vec<Opening>,
    pub south: Vec<Opening>,
}

/// One opening: its span along the wall's `u` and its courses `[bottom, head)` from the foundation.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opening {
    pub u: (f64, f64),
    pub courses: (u32, u32),
}

/// The fracture bake's cut rule.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cuts {
    pub row_courses: u32,
    pub crack_chance: f64,
    pub crack_shift_m: f64,
    pub min_width_m: f64,
    pub board_piece_m: (f64, f64),
    pub plate_piece_m: (f64, f64),
}

impl House {
    /// Parse a spec's text, refusing one whose numbers describe no house.
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let house: Self = ron::from_str(text).map_err(|err| err.to_string())?;
        house.check()?;
        Ok(house)
    }

    fn check(&self) -> Result<(), String> {
        let positive = |v: f64| v.is_finite() && v > 0.0;
        for (name, value) in [
            ("width_m", self.width_m),
            ("length_m", self.length_m),
            ("wall_thickness_m", self.wall_thickness_m),
            ("course_m", self.course_m),
            ("module_m", self.module_m),
            ("plate_height_m", self.plate_height_m),
            ("board_width_m", self.board_width_m),
            ("board_thickness_m", self.board_thickness_m),
            ("crack_shift_m", self.cuts.crack_shift_m),
            ("min_width_m", self.cuts.min_width_m),
        ] {
            if !positive(value) {
                return Err(format!("{name} {value} is not a positive length"));
            }
        }
        if 2.0 * self.wall_thickness_m >= self.width_m.min(self.length_m) {
            return Err("the walls leave no inside".into());
        }
        if self.pitch.0 == 0 || self.pitch.1 == 0 || self.boards_per_slope == 0 {
            return Err("a roof needs a pitch and boards".into());
        }
        if self.cuts.row_courses == 0 || !(0.0..=1.0).contains(&self.cuts.crack_chance) {
            return Err("the cut rule needs rows and a crack chance in [0, 1]".into());
        }
        for (name, (low, high)) in [
            ("board_piece_m", self.cuts.board_piece_m),
            ("plate_piece_m", self.cuts.plate_piece_m),
        ] {
            if !(positive(low) && low <= high) {
                return Err(format!("{name} ({low}, {high}) is no length range"));
            }
        }
        for (k, join) in self.joins.iter().enumerate() {
            if self.joins[..k].iter().any(|other| other.joins(join.kinds)) {
                return Err(format!("the join of {:?} is stated twice", join.kinds));
            }
            if let Hold::Fixed(name) = &join.by
                && !self.fixings.contains_key(name)
            {
                return Err(format!(
                    "the join of {:?} names fixing {name:?}, which is not stated",
                    join.kinds
                ));
            }
        }
        for (name, fixing) in &self.fixings {
            if !self
                .joins
                .iter()
                .any(|join| join.by == Hold::Fixed(name.clone()))
            {
                return Err(format!("fixing {name:?} holds no join"));
            }
            if let Fixing::Stated {
                tensile_pa,
                fracture_energy_j_per_m2,
            } = *fixing
                && !(positive(tensile_pa) && positive(fracture_energy_j_per_m2))
            {
                return Err(format!("fixing {name:?} states no strength"));
            }
        }
        for (wall, openings) in self.walls.iter() {
            for opening in openings {
                let (bottom, head) = opening.courses;
                if !(opening.u.0 < opening.u.1 && bottom < head && head <= self.eaves_course) {
                    return Err(format!(
                        "{wall} has an opening that is empty or above the eaves"
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Walls {
    /// Every wall's openings, in the fixed order the bake emits walls in.
    pub fn iter(&self) -> [(&'static str, &[Opening]); 4] {
        [
            ("east", &self.east),
            ("west", &self.west),
            ("north", &self.north),
            ("south", &self.south),
        ]
    }
}
