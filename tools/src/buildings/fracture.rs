//! The common cutter: one convex piece into convex chunks that tile it, by the [`Pattern`] its
//! construction data names. Nothing here reads what a piece is made of: the pattern, its scale and
//! its stretch are how a break looks and how finely the laws resolve it, never how strong anything
//! is (strength is the material's row and the attachments' list). Every cut is a plane through the
//! piece's [`Polyhedron`], so two chunks either side of one cut share its plane exactly.
//!
//! # Whole
//!
//! One chunk: the piece itself.
//!
//! # Courses: rows on bed joints, cuts that prefer head joints
//!
//! Masonry in running bond, in the bond's own frame ([`Courses`]): courses `course_m` tall up from
//! the datum, each course's head joints every `module_m` along it from the datum, every other
//! course offset by half a module. The cut follows that bond:
//!
//! 1. **Rows.** Row boundaries lie on bed joints, every `row_courses` courses counted from the
//!    datum, so pieces sharing a datum (a wall's piers, sills, lintels and gable) line their rows
//!    up. A piece's top row shorter than half a row (a gable's apex) joins the row below.
//! 2. **Cuts.** Each row's candidate cuts are the head joints of its bottom course, kept where the
//!    chunk on either side stays at least `min_width_m` wide at both row ends. A row of
//!    three courses then cuts its top course on joints too and its middle course through half a
//!    unit, and the rows' cuts alternate by half a module, so the chunks lie in a running bond.
//! 3. **Cracks.** Each candidate draws from a stream seeded by the asset, the piece and its row and
//!    joint: with `crack_chance` it leaves the joint, each end moving up to `crack_shift_m`, as a
//!    straight crack through the units. A crack that would leave a chunk narrower than
//!    `min_width_m` stays on its joint.
//!
//! # Cells: Voronoi cells stretched along the grain
//!
//! One site per cell of a grid `size_m` across the grain and `size_m × stretch` along it, drawn
//! uniformly inside its grid cell from a stream seeded by the asset, the piece and the grid cell; a
//! site outside the piece is dropped. Each chunk is the part of the piece nearer its site than any
//! other site, distance along the grain counting `1 / stretch` of distance across it, so a chunk
//! runs about `stretch` times longer along the grain. The grain is the piece's surface frame's
//! ([`grain`]). A piece holding fewer than two sites is one chunk.

use glam::DVec3;
use overmatch::buildings::schema::SurfaceFrame;
use serde::Deserialize;

use super::geom::{Polyhedron, WELD_M, plane_basis};
use super::spec::{Choice, Piece};

/// A deterministic stream: splitmix64 over a seed folded from text by FNV-1a, so the same names
/// draw the same numbers on every platform.
pub struct Stream(u64);

impl Stream {
    /// The stream named by `parts`, each part's bytes then a separator folded in.
    pub fn new(parts: &[&str]) -> Self {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for part in parts {
            for &byte in part.as_bytes().iter().chain(&[0xff]) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        Self(hash)
    }

    /// The next draw, uniform on `[0, 1)` with 53 bits.
    pub fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// How a piece is cut (module doc). Named by construction data, never inferred from the piece's
/// material, kind or fastening.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub enum Pattern {
    Whole,
    Courses(Courses),
    Cells(Cells),
}

/// A running bond (module doc).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Courses {
    /// A point on the datum bed joint at a head joint, building frame.
    pub origin: DVec3,
    /// Along the courses, unit.
    pub along: DVec3,
    /// Up across the courses, unit and perpendicular to `along`.
    pub up: DVec3,
    /// One course's height, m: the masonry unit's, with its joint.
    pub course_m: f64,
    /// One unit's length along the course, m, with its joint.
    pub module_m: f64,
    pub rule: CourseRule,
}

/// Where a running bond's chunks fall (module doc): appearance and resolution, never strength.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CourseRule {
    /// Courses per row of chunks.
    pub row_courses: Choice<u32>,
    /// The chance a cut leaves its head joint as a crack, in `[0, 1]`.
    pub crack_chance: Choice<f64>,
    /// How far each end of a crack may move off its joint, m.
    pub crack_shift_m: Choice<f64>,
    /// The least clearance a cut must leave at a row's top and bottom, m.
    pub min_width_m: Choice<f64>,
}

/// Voronoi cells stretched along the grain (module doc): appearance and resolution, never strength.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cells {
    /// A cell's size across the grain, m.
    pub size_m: Choice<f64>,
    /// A cell's length along the grain over its size across it.
    pub stretch: Choice<f64>,
}

/// The axis a piece's grain runs along: its surface frame's grain, or `+Y` for an upright frame,
/// whose image runs up the building.
pub fn grain(frame: SurfaceFrame) -> DVec3 {
    match frame {
        SurfaceFrame::Upright => DVec3::Y,
        SurfaceFrame::Grain([x, y, z]) => DVec3::new(x.into(), y.into(), z.into()).normalize(),
    }
}

/// How far an authored bond axis may sit from unit length or from perpendicular (Resolution: far
/// above a decimal's f64 rounding, far below any visible skew of the bond).
const UNIT: f64 = 1.0e-9;

fn positive(name: &str, value: f64) -> Result<(), String> {
    (value.is_finite() && value > 0.0)
        .then_some(())
        .ok_or_else(|| format!("{name} {value} is not finite and positive"))
}

impl Pattern {
    /// Refuses a pattern that cuts nothing finite or states a value with no reason, naming the
    /// field.
    pub fn check(&self) -> Result<(), String> {
        match self {
            Self::Whole => Ok(()),
            Self::Courses(courses) => {
                for (name, axis) in [("along", courses.along), ("up", courses.up)] {
                    if !(axis.is_finite() && (axis.length() - 1.0).abs() <= UNIT) {
                        return Err(format!("Courses {name} {axis} is not a unit axis"));
                    }
                }
                if courses.along.dot(courses.up).abs() > UNIT || !courses.origin.is_finite() {
                    return Err("Courses along and up must be perpendicular, origin finite".into());
                }
                positive("Courses course_m", courses.course_m)?;
                positive("Courses module_m", courses.module_m)?;
                courses.rule.check()
            }
            Self::Cells(cells) => cells.check(),
        }
    }
}

impl Cells {
    /// Refuses a size or stretch that is not finite and positive or has no reason.
    pub fn check(&self) -> Result<(), String> {
        self.size_m.check("Cells size_m")?;
        self.stretch.check("Cells stretch")?;
        positive("Cells size_m", self.size_m.0)?;
        positive("Cells stretch", self.stretch.0)
    }
}

impl CourseRule {
    /// Refuses a rule with no rows, a chance outside `[0, 1]`, a length that is not positive or a
    /// value with no reason, naming the field.
    pub fn check(&self) -> Result<(), String> {
        self.row_courses.check("row_courses")?;
        self.crack_chance.check("crack_chance")?;
        self.crack_shift_m.check("crack_shift_m")?;
        self.min_width_m.check("min_width_m")?;
        if self.row_courses.0 == 0 || !(0.0..=1.0).contains(&self.crack_chance.0) {
            return Err("row_courses must be positive and crack_chance in [0, 1]".into());
        }
        positive("crack_shift_m", self.crack_shift_m.0)?;
        positive("min_width_m", self.min_width_m.0)
    }
}

/// How the course cuts fell: on head joints, or as cracks through units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CutCount {
    pub joints: usize,
    pub cracks: usize,
}

/// The chunks of `piece`, in its pattern's order (courses: rows from the datum up, then along the
/// course; cells: their grid's order). Adds the course cuts to `count`; `asset` and the piece's
/// name seed every draw.
pub fn chunks(piece: &Piece, asset: &str, count: &mut CutCount) -> Vec<Polyhedron> {
    match &piece.pattern {
        Pattern::Whole => vec![piece.solid.clone()],
        Pattern::Courses(bond) => courses(piece, asset, bond, count),
        Pattern::Cells(rule) => cells(piece, asset, rule),
    }
}

/// One cut through a row: its coordinate along the course at the row's bottom and at its top.
#[derive(Debug, Clone, Copy)]
struct Cut {
    bottom: f64,
    top: f64,
}

/// The coursed chunks of `piece` (module doc). Coordinates are `along · p` and `up · p`, the datum
/// at `along · origin` and `up · origin`.
fn courses(piece: &Piece, asset: &str, bond: &Courses, count: &mut CutCount) -> Vec<Polyhedron> {
    let solid = &piece.solid;
    let rule = &bond.rule;
    let (along, up) = (bond.along, bond.up);
    let (base_y, start) = (up.dot(bond.origin), along.dot(bond.origin));
    let (course, module, min_width) = (bond.course_m, bond.module_m, rule.min_width_m.0);
    let (y_min, y_max) = solid.extent(up);
    let row = f64::from(rule.row_courses.0) * course;
    // A bed joint within the weld of the piece's top or bottom is that face, not a row.
    let mut bounds = vec![y_min];
    let mut m = ((y_min - base_y) / row).floor() as i64 + 1;
    loop {
        let y = base_y + m as f64 * row;
        if y >= y_max - WELD_M {
            break;
        }
        if y > y_min + WELD_M {
            bounds.push(y);
        }
        m += 1;
    }
    bounds.push(y_max);
    if bounds.len() > 2 && y_max - bounds[bounds.len() - 2] < 0.5 * row {
        bounds.remove(bounds.len() - 2);
    }
    let mut out = Vec::new();
    for (r, pair) in bounds.windows(2).enumerate() {
        let (y0, y1) = (pair[0], pair[1]);
        let within = "a row lies between the piece's lowest and highest corners";
        let band = solid
            .clip(-up, -y0)
            .and_then(|band| band.clip(up, y1))
            .expect(within);
        let (lo, hi) = solid
            .section_extent(up, 0.5 * (y0 + y1), along)
            .expect(within);
        let (lo, hi) = (lo + min_width, hi - min_width);
        let limits = [y0, y1].map(|y| {
            let (lo, hi) = solid.section_extent(up, y, along).expect(within);
            (lo + min_width, hi - min_width)
        });
        let parity = (((y0 - base_y) / course).round() as i64).rem_euclid(2) as f64;
        let mut cuts: Vec<Cut> = Vec::new();
        let first = ((lo - start) / module - 0.5 * parity).ceil() as i64;
        let last = ((hi - start) / module - 0.5 * parity).floor() as i64;
        for j in first..=last {
            let joint = start + (j as f64 + 0.5 * parity) * module;
            let mut stream = Stream::new(&[
                asset,
                &piece.name,
                "row",
                &r.to_string(),
                "joint",
                &j.to_string(),
            ]);
            let fits = |cut: Cut, cuts: &[Cut]| {
                let clear = cuts.last().is_none_or(|before| {
                    cut.bottom - before.bottom >= min_width && cut.top - before.top >= min_width
                });
                clear
                    && [cut.bottom, cut.top]
                        .into_iter()
                        .zip(limits)
                        .all(|(u, (lo, hi))| u >= lo && u <= hi)
            };
            let seam = Cut {
                bottom: joint,
                top: joint,
            };
            let cut = if stream.unit() < rule.crack_chance.0 {
                let shift =
                    |stream: &mut Stream| (2.0 * stream.unit() - 1.0) * rule.crack_shift_m.0;
                let crack = Cut {
                    bottom: joint + shift(&mut stream),
                    top: joint + shift(&mut stream),
                };
                fits(crack, &cuts).then_some(crack)
            } else {
                None
            };
            match cut {
                Some(crack) => {
                    count.cracks += 1;
                    cuts.push(crack);
                }
                None if fits(seam, &cuts) => {
                    count.joints += 1;
                    cuts.push(seam);
                }
                None => {}
            }
        }
        // Each cut runs from `bottom` at `y0` to `top` at `y1`; the plane through it and the axis
        // `along × up` keeps its left side as `n · p ≤ c`. Both ends clear the row boundary by
        // `min_width`, so convexity keeps the whole cut inside and leaves a chunk either side.
        let side = |cut: &Cut| {
            let (du, dy) = (cut.top - cut.bottom, y1 - y0);
            let n = along * dy - up * du;
            (n, dy * cut.bottom - du * y0)
        };
        let crossed = "every cut crosses its row inside the piece";
        for k in 0..=cuts.len() {
            let mut chunk = band.clone();
            if k > 0 {
                let (n, c) = side(&cuts[k - 1]);
                chunk = chunk.clip(-n, -c).expect(crossed);
            }
            if k < cuts.len() {
                let (n, c) = side(&cuts[k]);
                chunk = chunk.clip(n, c).expect(crossed);
            }
            out.push(chunk);
        }
    }
    out
}

fn cells(piece: &Piece, asset: &str, rule: &Cells) -> Vec<Polyhedron> {
    let solid = &piece.solid;
    let (size, stretch) = (rule.size_m.0, rule.stretch.0);
    let grain = grain(piece.surface_frame);
    let (e1, e2) = plane_basis(grain);
    let axes = [(e1, size), (e2, size), (grain, size * stretch)];
    let range = axes.map(|(axis, pitch)| {
        let (lo, hi) = solid.extent(axis);
        ((lo / pitch).floor() as i64, (hi / pitch).ceil() as i64)
    });
    let inside = |p: DVec3| {
        solid
            .faces
            .iter()
            .all(|face| face.normal.dot(p) < face.offset)
    };
    let mut sites = Vec::new();
    for i in range[0].0..range[0].1 {
        for j in range[1].0..range[1].1 {
            for l in range[2].0..range[2].1 {
                let cell = [i, j, l].map(|k| k.to_string());
                let mut stream =
                    Stream::new(&[asset, &piece.name, "cell", &cell[0], &cell[1], &cell[2]]);
                let site = [i, j, l]
                    .iter()
                    .zip(axes)
                    .map(|(&k, (axis, pitch))| axis * ((k as f64 + stream.unit()) * pitch))
                    .sum::<DVec3>();
                if inside(site) {
                    sites.push(site);
                }
            }
        }
    }
    if sites.len() < 2 {
        return vec![solid.clone()];
    }
    // The stretched metric `M v = v + (1/stretch² − 1)(g · v) g`: a site `b` takes from `a`'s cell
    // the half-space `M(b − a) · p > (b·Mb − a·Ma) / 2`. `M(−v)` is `−M(v)` to the bit, so two
    // neighbours cut on one plane, facing opposite ways.
    let shrink = 1.0 / (stretch * stretch) - 1.0;
    let metric = |v: DVec3| v + grain * (shrink * grain.dot(v));
    let square = |v: DVec3| v.dot(metric(v));
    sites
        .iter()
        .enumerate()
        .map(|(k, &a)| {
            let mut others: Vec<(f64, DVec3)> = sites
                .iter()
                .enumerate()
                .filter(|&(m, _)| m != k)
                .map(|(_, &b)| (square(b - a), b))
                .collect();
            others.sort_by(|x, y| x.0.total_cmp(&y.0));
            let mut cell = solid.clone();
            for (apart, b) in others {
                // A site twice the cell's reach away or more is nearer no point of it than `a`.
                let reach = cell
                    .corners
                    .iter()
                    .map(|&p| square(p - a))
                    .fold(0.0, f64::max);
                if apart > 4.0 * reach {
                    break;
                }
                cell = cell
                    .clip(metric(b - a), 0.5 * (square(b) - square(a)))
                    .expect("a site lies strictly inside its own cell");
            }
            cell
        })
        .collect()
}
