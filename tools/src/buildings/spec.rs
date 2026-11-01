//! `tools/buildings/<id>.house.ron`: the house generator's input. Read by the bake only; the game
//! never sees it.

use overmatch::buildings::attachments::Attachments;
use overmatch::buildings::schema::{ElementKind, SurfaceFrame};
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
    /// What each part is made of.
    pub materials: PartMaterials,
    pub surface_frames: PartSurfaceFrames,
    pub walls: Walls,
    /// What holds the parts of two components where they touch, by their elements' kinds.
    pub joins: Vec<Join>,
    pub cuts: Cuts,
}

/// Authored mapping and grain per construction part, in the building frame.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartSurfaceFrames {
    pub walls: SurfaceFrame,
    pub plates: SurfaceFrame,
    pub roof: SurfaceFrame,
}

/// What each part of the house is made of, as keys of `assets/materials/materials.ron`. The parts'
/// construction is the generator's: the walls are coursed masonry (`course_m`, `module_m`), the
/// plates and the roof timber (`board_*`, the cut rule's pieces); which matter each is built of is
/// this house's.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartMaterials {
    /// The masonry unit of every wall: piers, sills, lintels and gables.
    pub walls: Material,
    /// The wall plates on the long walls' tops.
    pub plates: Material,
    /// The roof planes' boards.
    pub roof: Material,
}

/// What holds every joint between an element of one of `kinds` and an element of the other (in
/// either order) where the two lie in different components: the attachment kind `by` names
/// (`assets/materials/attachments.ron`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Join {
    pub kinds: (ElementKind, ElementKind),
    pub by: String,
}

impl Join {
    /// Whether this join holds the pair of kinds `(a, b)`, in either order.
    pub fn joins(&self, (a, b): (ElementKind, ElementKind)) -> bool {
        self.kinds == (a, b) || self.kinds == (b, a)
    }
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
    /// Parse a spec's text, refusing one whose numbers describe no house, and a join naming an
    /// attachment kind the shared list does not hold.
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let house: Self = ron::from_str(text).map_err(|err| err.to_string())?;
        house.check()?;
        Ok(house)
    }

    pub(super) fn check(&self) -> Result<(), String> {
        for (part, frame) in [
            ("walls", self.surface_frames.walls),
            ("plates", self.surface_frames.plates),
            ("roof", self.surface_frames.roof),
        ] {
            if !frame.is_valid() {
                return Err(format!(
                    "surface_frames.{part}: author Upright or a finite unit Grain direction"
                ));
            }
        }
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
            if !(positive(low) && positive(high) && low <= high) {
                return Err(format!("{name} ({low}, {high}) is no length range"));
            }
        }
        for (k, join) in self.joins.iter().enumerate() {
            if self.joins[..k].iter().any(|other| other.joins(join.kinds)) {
                return Err(format!("the join of {:?} is stated twice", join.kinds));
            }
            Attachments::shipped_static()
                .holds(&join.by)
                .map_err(|err| format!("the join of {:?}, by: {err}", join.kinds))?;
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
            // The generator splits the wall into a pier before, between and after the openings,
            // in `u` order: each one lies inside the wall's face with a pier of some width on
            // either side, so no two overlap or touch and none reaches a corner.
            let (start, end) = self.wall_span(wall);
            let mut spans: Vec<(f64, f64)> = openings.iter().map(|opening| opening.u).collect();
            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut from = start;
            for (low, high) in spans {
                if !(low.is_finite() && high.is_finite() && from < low) {
                    return Err(format!(
                        "{wall} has an opening at u {low}..{high} that leaves no pier before it \
                         (the wall's face runs {start}..{end}; openings may not overlap)"
                    ));
                }
                from = high;
            }
            if from >= end {
                return Err(format!(
                    "{wall}'s last opening ends at u {from}, leaving no pier before the wall's \
                     end at {end}"
                ));
            }
        }
        Ok(())
    }

    /// The `u` range of `wall`'s face, the axis its openings are placed along: the long walls (east,
    /// west) run the full length, and the gable walls (north, south) stand between their inner
    /// faces.
    pub fn wall_span(&self, wall: &str) -> (f64, f64) {
        match wall {
            "east" | "west" => (-0.5 * self.length_m, 0.5 * self.length_m),
            "north" | "south" => {
                let inner = 0.5 * self.width_m - self.wall_thickness_m;
                (-inner, inner)
            }
            other => unreachable!("`Walls::iter` names four walls, not {other:?}"),
        }
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
