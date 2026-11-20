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
//! # Planar: cells through the depth
//!
//! StandIn for a rate-dependent fracture law of masonry the game does not have: under a slow load
//! a crack steps along the mortar joints, under an impulsive one (a burst, a ram) it runs through
//! the units as readily, so the cut owes nothing to the courses. One candidate site per square of
//! a grid `pitch_m` on a side across [`Planar::depth`], drawn uniformly inside its square from a
//! stream seeded by the asset, the piece and the square, kept with chance `keep_share`; a site
//! outside the piece is dropped. Every site lies on the piece's mid-depth plane, so every cut, the
//! plane bisecting two sites, runs through the piece's whole depth. A dropped site's square goes
//! to its neighbours, which makes the few large chunks; two sites drawn close make small ones.
//!
//! # Cells: Voronoi cells stretched along the grain
//!
//! One site per cell of a grid `size_m` across the grain and `size_m × stretch` along it, drawn
//! uniformly inside its grid cell from a stream seeded by the asset, the piece and the grid cell; a
//! site outside the piece is dropped. Distance along the grain counts `1 / stretch` of distance
//! across it, so a chunk runs about `stretch` times longer along the grain. The grain is the
//! piece's surface frame's ([`grain`]).
//!
//! # Both
//!
//! Each chunk is the part of the piece nearer its site than any other site ([`voronoi`]); a piece
//! holding fewer than two sites is one chunk. A planar site whose cell would hold a sliver is
//! dropped and its matter goes to its neighbours ([`SLIVER_M`]). Stretched cells keep theirs: a
//! Euclidean corner distance is no distance in their metric, and dropping by it would break their
//! affine law (a stretched piece's cells are its unstretched cells, stretched).

use glam::DVec3;
use overmatch::construction::schema::SurfaceFrame;
use serde::Deserialize;

use super::geom::{Polyhedron, plane_basis};
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
    Planar(Planar),
    Cells(Cells),
}

/// Cells through the depth (module doc).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Planar {
    /// The axis every cut runs along: a wall's thickness, unit.
    pub depth: DVec3,
    pub rule: PlanarRule,
}

/// Where planar cuts fall (module doc): appearance and resolution, never strength.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanarRule {
    /// The site grid's square, m.
    pub pitch_m: Choice<f64>,
    /// The share of squares that keep their site, in `(0, 1]`.
    pub keep_share: Choice<f64>,
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
/// whose image runs up the construction.
pub fn grain(frame: SurfaceFrame) -> DVec3 {
    match frame {
        SurfaceFrame::Upright => DVec3::Y,
        SurfaceFrame::Grain([x, y, z]) => DVec3::new(x.into(), y.into(), z.into()).normalize(),
    }
}

/// How far an authored axis may sit from unit length (Resolution: far above a decimal's f64
/// rounding, far below any visible skew of the cut).
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
            Self::Planar(planar) => {
                let depth = planar.depth;
                if !(depth.is_finite() && (depth.length() - 1.0).abs() <= UNIT) {
                    return Err(format!("Planar depth {depth} is not a unit axis"));
                }
                planar.rule.check()
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

impl PlanarRule {
    /// Refuses a pitch that is not finite and positive, a share outside `(0, 1]` or a value with
    /// no reason, naming the field.
    pub fn check(&self) -> Result<(), String> {
        self.pitch_m.check("Planar pitch_m")?;
        self.keep_share.check("Planar keep_share")?;
        positive("Planar pitch_m", self.pitch_m.0)?;
        let share = self.keep_share.0;
        (share > 0.0 && share <= 1.0)
            .then_some(())
            .ok_or_else(|| format!("Planar keep_share {share} is not in (0, 1]"))
    }
}

/// The chunks of `piece`, in its pattern's grid order; `asset` and the piece's name seed every
/// draw. Refuses a planar cut of a piece that is itself a sliver ([`SLIVER_M`]): no chunk of it
/// could be kept.
pub fn chunks(piece: &Piece, asset: &str) -> Result<Vec<Polyhedron>, String> {
    let solid = &piece.solid;
    let inside = |p: DVec3| {
        solid
            .faces
            .iter()
            .all(|face| face.normal.dot(p) < face.offset)
    };
    // Grid squares `[lo, hi)` along `axis` at `pitch` covering the solid.
    let range = |axis: DVec3, pitch: f64| {
        let (lo, hi) = solid.extent(axis);
        (lo / pitch).floor() as i64..(hi / pitch).ceil() as i64
    };
    match &piece.pattern {
        Pattern::Whole => Ok(vec![solid.clone()]),
        Pattern::Planar(_) if sliver(solid) => Err(format!(
            "{}: corners nearer than {SLIVER_M} m leave a Planar cut no chunk to keep; cut the \
             piece Whole or widen it",
            piece.name
        )),
        Pattern::Planar(Planar { depth, rule }) => {
            let (pitch, keep) = (rule.pitch_m.0, rule.keep_share.0);
            let (e1, e2) = plane_basis(*depth);
            let (near, far) = solid.extent(*depth);
            let (ri, rj) = (range(e1, pitch), range(e2, pitch));
            let mut sites = Vec::new();
            for i in ri {
                for j in rj.clone() {
                    let square = [i, j].map(|k| k.to_string());
                    let mut stream =
                        Stream::new(&[asset, &piece.name, "planar", &square[0], &square[1]]);
                    let site = e1 * ((i as f64 + stream.unit()) * pitch)
                        + e2 * ((j as f64 + stream.unit()) * pitch)
                        + *depth * (0.5 * (near + far));
                    if stream.unit() < keep && inside(site) {
                        sites.push(site);
                    }
                }
            }
            Ok(without_slivers(solid, sites))
        }
        Pattern::Cells(rule) => {
            let (size, stretch) = (rule.size_m.0, rule.stretch.0);
            let grain = grain(piece.surface_frame);
            let (e1, e2) = plane_basis(grain);
            let axes = [(e1, size), (e2, size), (grain, size * stretch)];
            let [ri, rj, rl] = axes.map(|(axis, pitch)| range(axis, pitch));
            let mut sites = Vec::new();
            for i in ri {
                for j in rj.clone() {
                    for l in rl.clone() {
                        let cell = [i, j, l].map(|k| k.to_string());
                        let mut stream = Stream::new(&[
                            asset,
                            &piece.name,
                            "cell",
                            &cell[0],
                            &cell[1],
                            &cell[2],
                        ]);
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
            let shrink = 1.0 / (stretch * stretch) - 1.0;
            Ok(voronoi(solid, &sites, |v| {
                v + grain * (shrink * grain.dot(v))
            }))
        }
    }
}

/// Resolution: two corners of one chunk closer than this (m) bound a sliver, dropped with its
/// site (module doc). A freed chunk's joint faces stand 1 mm back for contact
/// (`overmatch::construction::breakage::LOOSE_GAP_M`), which swallows a face a few of those
/// across; far below a chunk's size at any authored pitch.
pub const SLIVER_M: f64 = 5e-3;

/// The cells of `sites` in `solid`, a site whose cell holds a sliver dropped, the first in order
/// first, until none does. `solid` is no sliver itself.
pub(super) fn without_slivers(solid: &Polyhedron, mut sites: Vec<DVec3>) -> Vec<Polyhedron> {
    loop {
        let cells = voronoi(solid, &sites, |v| v);
        match cells.iter().position(sliver) {
            None => return cells,
            Some(_) if sites.len() < 2 => {
                unreachable!("fewer than two sites leave the solid, which is no sliver")
            }
            Some(k) => sites.remove(k),
        };
    }
}

/// Whether two of `cell`'s corners lie nearer than [`SLIVER_M`].
pub(super) fn sliver(cell: &Polyhedron) -> bool {
    let corners = &cell.corners;
    (0..corners.len()).any(|i| {
        corners[i + 1..]
            .iter()
            .any(|q| corners[i].distance(*q) < SLIVER_M)
    })
}

/// The cells of `sites` in `solid` (module doc), in their order, by distance in the self-adjoint
/// linear `metric` `M`: a site `b` takes from `a`'s cell the half-space `M(b − a) · p > (b·Mb −
/// a·Ma) / 2`. `M(−v)` is `−M(v)` to the bit, so two neighbours cut on one plane, facing opposite
/// ways. Fewer than two sites leave the solid whole.
pub(super) fn voronoi(
    solid: &Polyhedron,
    sites: &[DVec3],
    metric: impl Fn(DVec3) -> DVec3,
) -> Vec<Polyhedron> {
    if sites.len() < 2 {
        return vec![solid.clone()];
    }
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
