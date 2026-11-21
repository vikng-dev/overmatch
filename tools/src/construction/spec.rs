//! The bake's construction data, read by the bake only; the game never sees it. Two authored
//! forms resolve to one [`Construction`], the common cutter's input:
//!
//! - `tools/constructions/<id>.house.ron`, a [`House`]: the house generator's parameters
//!   ([`super::house`]).
//! - `tools/constructions/<id>.pieces.ron`, a [`ConstructionSpec`]: convex pieces stated
//!   outright, so a new object needs data, not a generator.
//!
//! Either way each piece names its material (a key of `assets/materials/materials.ron`), its
//! surface frame, its component and its cut [`Pattern`], and the joins name the attachment kind
//! between components (`assets/materials/attachments.ron`). The bake infers none of these from
//! another.

use glam::DVec3;
use overmatch::construction::attachments::Attachments;
use overmatch::construction::schema::SurfaceFrame;
use serde::Deserialize;

use super::fracture::{Pattern, PlanarRule};
use super::geom::Polyhedron;

/// A cut parameter and its job, authored `(value, Taste("…"))` or `(value, Resolution("…"))`: how
/// a break looks or how finely the laws resolve it. A strength claim does not parse here; that
/// belongs to a material's row.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Choice<T>(pub T, pub Job);

/// What a [`Choice`] does, with its non-blank reason.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub enum Job {
    /// A look; names the effect.
    Taste(String),
    /// How finely or stably a law reads the result; gives the cost or stability reason.
    Resolution(String),
    /// A test's inline data; refused in a shipped spec ([`Shipped`]).
    #[cfg(test)]
    Fixture(String),
}

#[cfg(test)]
thread_local! {
    static SHIPPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// While held, specs read on this thread are shipped files, whose `Fixture` labels are refused in
/// the test build as in every other: [`super::specs`] holds one.
pub(super) struct Shipped {
    #[cfg(test)]
    was: bool,
}

impl Shipped {
    pub(super) fn enter() -> Self {
        Self {
            #[cfg(test)]
            was: SHIPPED.replace(true),
        }
    }
}

#[cfg(test)]
impl Drop for Shipped {
    fn drop(&mut self) {
        SHIPPED.set(self.was);
    }
}

impl<T> Choice<T> {
    /// Refuses a blank reason, and a fixture's in a shipped spec, naming `field`.
    pub fn check(&self, field: &str) -> Result<(), String> {
        let note = match &self.1 {
            Job::Taste(note) | Job::Resolution(note) => note,
            #[cfg(test)]
            Job::Fixture(_) if SHIPPED.get() => {
                return Err(format!(
                    "{field}: Fixture is a test's label; a shipped spec names its Taste or \
                     Resolution"
                ));
            }
            #[cfg(test)]
            Job::Fixture(note) => note,
        };
        if note.trim().is_empty() {
            return Err(format!("{field}: its Taste or Resolution names no reason"));
        }
        Ok(())
    }
}

/// One construction as the cutter reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Construction {
    pub id: String,
    /// The foundation plane's height, m: every downward chunk face on it is an anchor.
    pub foundation_y: f64,
    /// The attachment kind holding every anchor.
    pub foundation_attachment: String,
    /// In emission order: the written elements' order.
    pub pieces: Vec<Piece>,
    pub joins: Vec<Join>,
}

/// One piece: a written element, cut by its pattern into the chunks that tile it.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    /// Unique within the construction; seeds its cut's draws.
    pub name: String,
    pub part: String,
    /// The component it is part of: chunks inside one hold by their material's seam.
    pub component: String,
    /// A key of `assets/materials/materials.ron`.
    pub material: String,
    pub surface_frame: SurfaceFrame,
    pub solid: Polyhedron,
    pub pattern: Pattern,
}

/// What holds every joint between an element of one of `parts` and an element of the other (in
/// either order) where the two lie in different components: the attachment kind `by` names
/// (`assets/materials/attachments.ron`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Join {
    pub parts: (String, String),
    pub by: String,
}

impl Join {
    /// Whether this join holds the pair of part names `(a, b)`, in either order.
    pub fn joins(&self, (a, b): (&str, &str)) -> bool {
        (self.parts.0 == a && self.parts.1 == b) || (self.parts.0 == b && self.parts.1 == a)
    }
}

/// Refuses an anchor or join kind the shared list does not hold and a pair of part names joined twice.
fn check_holds(foundation_attachment: &str, joins: &[Join]) -> Result<(), String> {
    Attachments::shipped_static()
        .holds_anchor(foundation_attachment)
        .map_err(|err| format!("foundation_attachment: {err}"))?;
    for (k, join) in joins.iter().enumerate() {
        if join.parts.0.trim().is_empty() || join.parts.1.trim().is_empty() {
            return Err(format!("joins[{k}].parts: author two non-blank part names"));
        }
        if joins[..k]
            .iter()
            .any(|other| other.joins((&join.parts.0, &join.parts.1)))
        {
            return Err(format!("the join of {:?} is stated twice", join.parts));
        }
        Attachments::shipped_static()
            .holds(&join.by)
            .map_err(|err| format!("the join of {:?}, by: {err}", join.parts))?;
    }
    Ok(())
}

/// Refuses two pieces whose interiors meet (at the clip's weld resolution), naming both: a chunk's
/// mass and the contact search's joints count each volume and each interface once. Shared faces,
/// edges and corners are allowed.
pub(super) fn check_disjoint(pieces: &[Piece]) -> Result<(), String> {
    for (k, piece) in pieces.iter().enumerate() {
        for (j, other) in pieces[..k].iter().enumerate() {
            if other.solid.meets(&piece.solid) {
                return Err(format!(
                    "pieces[{k}] ({}) overlaps pieces[{j}] ({}): author disjoint interiors; \
                     shared boundaries are allowed",
                    piece.name, other.name
                ));
            }
        }
    }
    Ok(())
}

/// Refuses a surface frame that is no unit grain, naming `what`.
fn check_frame(what: &str, frame: SurfaceFrame) -> Result<(), String> {
    frame.is_valid().then_some(()).ok_or_else(|| {
        format!("{what}.surface_frame: author Upright or a finite unit Grain direction")
    })
}

/// `tools/constructions/<id>.pieces.ron`: convex pieces stated outright, authored
/// `Construction(..)`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename = "Construction", deny_unknown_fields)]
pub struct ConstructionSpec {
    pub id: String,
    pub foundation_y_m: f64,
    pub foundation_attachment: String,
    pub pieces: Vec<PieceSpec>,
    pub joins: Vec<Join>,
}

/// One authored convex piece ([`Piece`]).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PieceSpec {
    pub name: String,
    pub part: String,
    pub component: String,
    pub material: String,
    pub surface_frame: SurfaceFrame,
    /// Construction frame, m.
    pub corners: Vec<DVec3>,
    /// Corner indices, counter-clockwise seen from outside; together a closed convex solid.
    pub faces: Vec<Vec<usize>>,
    pub pattern: Pattern,
}

impl ConstructionSpec {
    /// Parse a construction's text and resolve it, refusing what [`Self::resolve`] refuses.
    pub fn from_ron(text: &str) -> Result<Construction, String> {
        ron::from_str::<Self>(text)
            .map_err(|err| err.to_string())?
            .resolve()
    }

    /// The construction its pieces state, refusing a piece whose name repeats, whose index leaves
    /// its corners, whose solid is no closed convex cell at `tolerance` m ([`Polyhedron::certify`]),
    /// whose frame or pattern is unusable, overlapping interiors (at the clip's weld resolution),
    /// and an attachment kind the shared list does not hold. Shared boundaries are allowed.
    pub fn resolve(self) -> Result<Construction, String> {
        check_holds(&self.foundation_attachment, &self.joins)?;
        if !self.foundation_y_m.is_finite() {
            return Err(format!(
                "foundation_y_m {} is not finite",
                self.foundation_y_m
            ));
        }
        let mut pieces = Vec::with_capacity(self.pieces.len());
        for (k, piece) in self.pieces.into_iter().enumerate() {
            let what = format!("pieces[{k}] ({})", piece.name);
            if pieces.iter().any(|other: &Piece| other.name == piece.name) {
                return Err(format!("{what}: its name is taken"));
            }
            if piece.part.trim().is_empty() {
                return Err(format!("{what}.part: author a non-blank part name"));
            }
            check_frame(&what, piece.surface_frame)?;
            piece
                .pattern
                .check()
                .map_err(|err| format!("{what}.pattern: {err}"))?;
            if piece
                .faces
                .iter()
                .flatten()
                .any(|&i| i >= piece.corners.len())
            {
                return Err(format!("{what}: a face names a corner past corners"));
            }
            let solid = Polyhedron::from_faces(piece.corners, piece.faces);
            solid
                .certify(super::PLANE_M)
                .map_err(|err| format!("{what}: no closed convex solid ({err})"))?;
            pieces.push(Piece {
                name: piece.name,
                part: piece.part,
                component: piece.component,
                material: piece.material,
                surface_frame: piece.surface_frame,
                solid,
                pattern: piece.pattern,
            });
        }
        check_disjoint(&pieces)?;
        Ok(Construction {
            id: self.id,
            foundation_y: self.foundation_y_m,
            foundation_attachment: self.foundation_attachment,
            pieces,
            joins: self.joins,
        })
    }
}

/// One house (the file's own comments carry each field's meaning and units).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct House {
    pub id: String,
    pub width_m: f64,
    pub length_m: f64,
    pub wall_thickness_m: f64,
    pub course_m: f64,
    pub foundation_courses: u32,
    /// Attachment holding every component's foundation footprint.
    pub foundation_attachment: String,
    pub eaves_course: u32,
    /// Rise over run, integers so every slope is an exact ratio.
    pub pitch: (u32, u32),
    pub plate_height_m: f64,
    pub board_width_m: f64,
    pub board_thickness_m: f64,
    pub boards_per_slope: u32,
    pub gable_overhang_m: f64,
    /// What each part is made of, how its surface runs and how it is cut.
    pub parts: Parts,
    pub walls: Walls,
    /// Interior walls across the ridge, each carrying a gable to the roof.
    pub cross_walls: Vec<CrossWall>,
    /// Timber framing under the boards; without it the boards lie on the plates and gables.
    pub roof_frame: Option<RoofFrame>,
    /// What holds the parts of two components where they touch, by their elements' part names.
    pub joins: Vec<Join>,
}

/// One interior wall across the ridge: its centre's `z`, its thickness and its openings, `u` along
/// `+X` between the long walls' inner faces.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossWall {
    pub name: String,
    pub at_z_m: f64,
    pub thickness_m: f64,
    pub openings: Vec<Opening>,
}

/// Timber framing under the roof boards, metres: a rafter pair over each gable-carrying wall's
/// centre line and at even centres between, bearing on the wall plates and on a ridge beam seated
/// in the gables. Each member's grain runs along its length.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoofFrame {
    /// A key of `assets/materials/materials.ron`.
    pub material: String,
    pub cut: PartCut,
    pub rafter_width_m: f64,
    /// Across the slope.
    pub rafter_depth_m: f64,
    /// The largest centre spacing of rafter pairs.
    pub rafter_spacing_m: f64,
    /// How far each rafter runs past its long wall's outer face.
    pub eaves_overhang_m: f64,
    pub ridge_width_m: f64,
    /// Below the roof's underside at the beam's faces.
    pub ridge_depth_m: f64,
}

/// The house's three parts: every wall's piers, sills, lintels and gable; the wall plates on the
/// long walls' tops; the roof planes. The generator lays them out; their matter, surface and cut
/// are this house's data.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parts {
    pub walls: Part,
    pub plates: Part,
    pub roof: Part,
}

/// One part's data.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    /// A key of `assets/materials/materials.ron`.
    pub material: String,
    /// Authored mapping and grain, in the construction frame.
    pub surface_frame: SurfaceFrame,
    pub cut: PartCut,
}

/// A house part's cut: a [`Pattern`] whose frame the generator supplies.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub enum PartCut {
    Whole,
    /// Cells through the element's depth, the wall's thickness.
    Planar(PlanarRule),
    Cells(super::fracture::Cells),
}

impl PartCut {
    fn check(&self) -> Result<(), String> {
        match self {
            Self::Whole => Ok(()),
            Self::Planar(rule) => rule.check(),
            Self::Cells(cells) => cells.check(),
        }
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

impl House {
    /// Parse a spec's text, refusing one whose numbers describe no house, and a join naming an
    /// attachment kind the shared list does not hold.
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let house: Self = ron::from_str(text).map_err(|err| err.to_string())?;
        house.check()?;
        Ok(house)
    }

    pub(super) fn check(&self) -> Result<(), String> {
        for (name, part) in [
            ("parts.walls", &self.parts.walls),
            ("parts.plates", &self.parts.plates),
            ("parts.roof", &self.parts.roof),
        ] {
            check_frame(name, part.surface_frame)?;
            (part.cut.check()).map_err(|err| format!("{name}.cut: {err}"))?;
        }
        let positive = |v: f64| v.is_finite() && v > 0.0;
        for (name, value) in [
            ("width_m", self.width_m),
            ("length_m", self.length_m),
            ("wall_thickness_m", self.wall_thickness_m),
            ("course_m", self.course_m),
            ("plate_height_m", self.plate_height_m),
            ("board_width_m", self.board_width_m),
            ("board_thickness_m", self.board_thickness_m),
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
        let inner_l = 0.5 * self.length_m - self.wall_thickness_m;
        for (i, wall) in self.cross_walls.iter().enumerate() {
            if wall.name.trim().is_empty()
                || ["east", "west", "north", "south"].contains(&wall.name.as_str())
                || self.cross_walls[..i].iter().any(|w| w.name == wall.name)
            {
                return Err(format!(
                    "cross_walls[{i}].name: author a unique name no outer wall has"
                ));
            }
            let (near, far) = (
                wall.at_z_m - wall.thickness_m / 2.0,
                wall.at_z_m + wall.thickness_m / 2.0,
            );
            if !(positive(wall.thickness_m) && near > -inner_l && far < inner_l)
                || self.cross_walls[..i].iter().any(|w| {
                    (w.at_z_m - wall.at_z_m).abs() <= (w.thickness_m + wall.thickness_m) / 2.0
                })
            {
                return Err(format!(
                    "cross_walls[{i}]: place a wall of positive thickness inside the gable walls, apart from the others"
                ));
            }
        }
        if let Some(frame) = &self.roof_frame {
            self.check_frame_members(frame)?;
        }
        check_holds(&self.foundation_attachment, &self.joins)?;
        let interior = 0.5 * self.width_m - self.wall_thickness_m;
        let walls = self
            .walls
            .iter()
            .into_iter()
            .chain((self.cross_walls.iter()).map(|w| (w.name.as_str(), w.openings.as_slice())));
        for (wall, openings) in walls {
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
            let (start, end) = if self.cross_walls.iter().any(|w| w.name == wall) {
                (-interior, interior)
            } else {
                self.wall_span(wall)
            };
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

    /// Refuses framing whose members cannot stand where the generator lays them: each
    /// gable-carrying wall must hold its rafter within its thickness, and the ridge beam's seat
    /// must lie in each gable, above the roof's underside at the long walls' inner faces.
    fn check_frame_members(&self, frame: &RoofFrame) -> Result<(), String> {
        frame
            .cut
            .check()
            .map_err(|err| format!("roof_frame.cut: {err}"))?;
        for (name, value) in [
            ("rafter_width_m", frame.rafter_width_m),
            ("rafter_depth_m", frame.rafter_depth_m),
            ("rafter_spacing_m", frame.rafter_spacing_m),
            ("ridge_width_m", frame.ridge_width_m),
            ("ridge_depth_m", frame.ridge_depth_m),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err(format!(
                    "roof_frame.{name} {value} is not a positive length"
                ));
            }
        }
        if !(frame.eaves_overhang_m.is_finite() && frame.eaves_overhang_m >= 0.0) {
            return Err("roof_frame.eaves_overhang_m: author a finite non-negative length".into());
        }
        let thinnest =
            (self.cross_walls.iter().map(|w| w.thickness_m)).fold(self.wall_thickness_m, f64::min);
        if frame.rafter_width_m >= thinnest {
            return Err(format!(
                "roof_frame.rafter_width_m: a rafter over a gable-carrying wall must lie within \
                 its {thinnest} m thickness"
            ));
        }
        // The seat's height over the roof's underside at the long walls' inner faces.
        let slope = f64::from(self.pitch.0) / f64::from(self.pitch.1);
        let inner = 0.5 * self.width_m - self.wall_thickness_m;
        let seat = slope * (inner - frame.ridge_width_m / 2.0) - frame.ridge_depth_m;
        if !(frame.ridge_width_m < 2.0 * inner && seat > 0.0) {
            return Err(
                "roof_frame.ridge_width_m/ridge_depth_m: seat the ridge beam in the gables: \
                 narrower than the inside, and shallower than the gable's rise over its faces"
                    .into(),
            );
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
