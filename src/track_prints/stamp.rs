//! The shoe's underside, baked once into a height-and-coverage stamp in the shoe's canonical frame
//! (`track::link_view::LinkFrame`: `x` lateral, `y` toward the inner face, `z` longitudinal). The
//! ground-facing side is `-y`, so the underside is the LOWEST surface over each `(x, z)`.
//!
//! [`bake`] is a pure function of canonical-frame triangles. It keeps a lower envelope, not a
//! picture: a cell holds the lowest solid point over it, measured up from the lowest point of the
//! whole shoe ([`ShoeStamp::datum_y`]), and a cell no triangle covers is an opening. Undercuts and
//! a second surface above the lowest are lost, which ordinary ground-facing shoes do not need.
//!
//! [`ClearanceStamp`] is the bake as the stores read it, prefiltered to a store texel: each cell
//! the mean height of the underside a texel-wide square about it covers, and open where that square
//! is less than half covered. The mean keeps a grouser's share of the pitch: a least-height filter
//! widens every grouser by a texel, which on a pitch under three texels fills the plate between
//! them and prints a flat sole.

use bevy::math::{UVec2, Vec2, Vec3, Vec3Swizzles};

/// The bake's cell, m. ASSUMED: about a tenth of the near store's texel, fine enough that the
/// texel-wide pooling decides what survives, not the bake.
pub(crate) const STAMP_CELL_M: f32 = 0.005;

/// Coverage samples per cell side: a cell's coverage is the fraction of its `2 × 2` samples a
/// triangle covers.
const SAMPLES: u32 = 2;

/// The height a [`ClearanceStamp`] holds over an opening, m: far above any print depth, so an
/// opening prints nothing on any pack.
pub(crate) const OPEN_M: f32 = 1.0;

/// The baked underside of one shoe: `R16` height over the datum and `R8` coverage per cell, with
/// the frame metadata that places it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ShoeStamp {
    /// Canonical `(x, z)` of cell `(0, 0)`'s low corner.
    pub(crate) origin: Vec2,
    /// Cell side, m.
    pub(crate) cell_m: f32,
    /// Cells across (`x`) and along (`z`).
    pub(crate) size: UVec2,
    /// Canonical `y` of the shoe's lowest underside point; every height is above it.
    pub(crate) datum_y: f32,
    /// Metres per height unit: `height · height_scale_m` is the underside's height over the datum.
    pub(crate) height_scale_m: f32,
    /// Row-major (`z` rows of `x` cells). `0` on a cell with no coverage.
    pub(crate) height: Vec<u16>,
    /// Row-major. `255` where every coverage sample of the cell lies under the shoe, `0` where none
    /// does: an opening.
    pub(crate) coverage: Vec<u8>,
    /// Triangles the bake read, for the log.
    pub(crate) triangles: usize,
}

impl ShoeStamp {
    /// The cell index of `(i, j)`.
    fn index(&self, i: u32, j: u32) -> usize {
        (j * self.size.x + i) as usize
    }

    /// The underside's height over the datum at cell `(i, j)`, m; `None` over an opening.
    pub(crate) fn height_at(&self, i: u32, j: u32) -> Option<f32> {
        let index = self.index(i, j);
        (self.coverage[index] > 0).then(|| f32::from(self.height[index]) * self.height_scale_m)
    }

    /// The cell holding canonical `(x, z)`, if it lies on the stamp.
    #[cfg(test)]
    pub(crate) fn cell_of(&self, x: f32, z: f32) -> Option<UVec2> {
        let local = (Vec2::new(x, z) - self.origin) / self.cell_m;
        (local.x >= 0.0 && local.y >= 0.0)
            .then(|| local.as_uvec2())
            .filter(|cell| cell.x < self.size.x && cell.y < self.size.y)
    }

    /// The stamp's canonical `(x, z)` bounds: low corner, high corner.
    pub(crate) fn bounds(&self) -> (Vec2, Vec2) {
        (self.origin, self.origin + self.size.as_vec2() * self.cell_m)
    }

    /// The same shoe reflected across its lateral mid-plane, `x → −x`: the stamp of a mesh that is
    /// this one's mirror ([`crate::track::link_view`]'s left shoe). The stores read the mirror
    /// through [`ClearanceStamp::height`] instead; this is the bake it must agree with.
    #[cfg(test)]
    pub(crate) fn mirrored(&self) -> Self {
        let (low, high) = self.bounds();
        let mut mirrored = self.clone();
        mirrored.origin = Vec2::new(-high.x, low.y);
        for j in 0..self.size.y {
            for i in 0..self.size.x {
                let from = self.index(self.size.x - 1 - i, j);
                let to = self.index(i, j);
                mirrored.height[to] = self.height[from];
                mirrored.coverage[to] = self.coverage[from];
            }
        }
        mirrored
    }
}

/// The underside of canonical-frame `triangles`, at [`STAMP_CELL_M`]. `None` when nothing covers
/// any cell.
pub(crate) fn bake(triangles: &[[Vec3; 3]]) -> Option<ShoeStamp> {
    bake_at(triangles, STAMP_CELL_M)
}

/// [`bake`] at a given cell size.
pub(crate) fn bake_at(triangles: &[[Vec3; 3]], cell_m: f32) -> Option<ShoeStamp> {
    let (mut low, mut high) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for vertex in triangles.iter().flatten() {
        low = low.min(vertex.xz());
        high = high.max(vertex.xz());
    }
    if !(low.x < high.x && low.y < high.y) {
        return None;
    }
    let size = ((high - low) / cell_m).ceil().as_uvec2().max(UVec2::ONE);
    // Centred on the bounds, so the grid of a mirrored shoe is this grid mirrored, cell for cell.
    let low = (low + high) * 0.5 - size.as_vec2() * cell_m * 0.5;
    let samples = size * SAMPLES;
    let step = cell_m / SAMPLES as f32;
    // The lowest `y` over each coverage sample; infinite where no triangle covers it.
    let mut lowest = vec![f32::INFINITY; (samples.x * samples.y) as usize];
    for &[a, b, c] in triangles {
        let (a2, b2, c2) = (a.xz(), b.xz(), c.xz());
        let area = (b2 - a2).perp_dot(c2 - a2);
        // A triangle seen edge-on from below covers nothing; the faces either side of it do.
        if area.abs() < 1e-12 {
            continue;
        }
        let sample = |v: f32, origin: f32| (v - origin) / step - 0.5;
        let lo = a2.min(b2).min(c2);
        let hi = a2.max(b2).max(c2);
        let i0 = sample(lo.x, low.x).ceil().max(0.0) as u32;
        let j0 = sample(lo.y, low.y).ceil().max(0.0) as u32;
        let i1 = (sample(hi.x, low.x).floor() as i64).min(i64::from(samples.x) - 1);
        let j1 = (sample(hi.y, low.y).floor() as i64).min(i64::from(samples.y) - 1);
        for j in i64::from(j0)..=j1 {
            for i in i64::from(i0)..=i1 {
                let p = low + (Vec2::new(i as f32, j as f32) + 0.5) * step;
                // Barycentric weights of `p`; it is inside when all three share the area's sign.
                let wa = (b2 - p).perp_dot(c2 - p) / area;
                let wb = (c2 - p).perp_dot(a2 - p) / area;
                let wc = 1.0 - wa - wb;
                if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                    continue;
                }
                let y = wa * a.y + wb * b.y + wc * c.y;
                let slot = &mut lowest[(j as u32 * samples.x + i as u32) as usize];
                *slot = slot.min(y);
            }
        }
    }
    let datum_y = lowest.iter().copied().fold(f32::INFINITY, f32::min);
    if !datum_y.is_finite() {
        return None;
    }
    let relief = lowest
        .iter()
        .copied()
        .filter(|y| y.is_finite())
        .fold(0.0_f32, |relief, y| relief.max(y - datum_y));
    let height_scale_m = relief.max(f32::EPSILON) / f32::from(u16::MAX);
    let cells = (size.x * size.y) as usize;
    let (mut height, mut coverage) = (vec![0_u16; cells], vec![0_u8; cells]);
    for j in 0..size.y {
        for i in 0..size.x {
            let (mut least, mut covered) = (f32::INFINITY, 0_u32);
            for sj in 0..SAMPLES {
                for si in 0..SAMPLES {
                    let y = lowest[((j * SAMPLES + sj) * samples.x + i * SAMPLES + si) as usize];
                    if y.is_finite() {
                        covered += 1;
                        least = least.min(y);
                    }
                }
            }
            let index = (j * size.x + i) as usize;
            if covered > 0 {
                height[index] = ((least - datum_y) / height_scale_m).round() as u16;
                coverage[index] = (covered * 255 / (SAMPLES * SAMPLES)) as u8;
            }
        }
    }
    Some(ShoeStamp {
        origin: low,
        cell_m,
        size,
        datum_y,
        height_scale_m,
        height,
        coverage,
        triangles: triangles.len(),
    })
}

/// A stamp pooled to a store's texel: each cell holds the coverage-weighted mean underside height
/// (m over the datum) of the cells within half a texel of it along both axes, [`OPEN_M`] where
/// they are less than half covered (the stamp's outside counts as open). Read by nearest cell
/// ([`Self::height`]); a mirrored shoe reads it at `(−x, z)`.
#[derive(Clone, Debug)]
pub(crate) struct ClearanceStamp {
    origin: Vec2,
    cell_m: f32,
    size: UVec2,
    height: Vec<f32>,
    /// The bake's datum, canonical `y`.
    datum_y: f32,
}

impl ClearanceStamp {
    /// `stamp` pooled over a `texel_m` square.
    pub(crate) fn pooled(stamp: &ShoeStamp, texel_m: f32) -> Self {
        let reach = ((0.5 * texel_m / stamp.cell_m).round() as i64).max(0);
        let window = ((2 * reach + 1) * (2 * reach + 1)) as f32;
        let (nx, nz) = (i64::from(stamp.size.x), i64::from(stamp.size.y));
        // Per cell: (covered fraction × height, covered fraction).
        let raw: Vec<(f32, f32)> = (0..stamp.size.y)
            .flat_map(|j| (0..stamp.size.x).map(move |i| (i, j)))
            .map(|(i, j)| {
                let index = stamp.index(i, j);
                let covered = f32::from(stamp.coverage[index]) / 255.0;
                (covered * stamp.height_at(i, j).unwrap_or(0.0), covered)
            })
            .collect();
        let add = |a: (f32, f32), b: (f32, f32)| (a.0 + b.0, a.1 + b.1);
        // Separable box sums: along x, then those along z.
        let mut across = vec![(0.0, 0.0); raw.len()];
        for j in 0..nz {
            for i in 0..nx {
                across[(j * nx + i) as usize] = (i - reach..=i + reach)
                    .filter(|&k| (0..nx).contains(&k))
                    .map(|k| raw[(j * nx + k) as usize])
                    .fold((0.0, 0.0), add);
            }
        }
        let mut height = vec![OPEN_M; raw.len()];
        for j in 0..nz {
            for i in 0..nx {
                let (weighted, covered) = (j - reach..=j + reach)
                    .filter(|&k| (0..nz).contains(&k))
                    .map(|k| across[(k * nx + i) as usize])
                    .fold((0.0, 0.0), add);
                if covered >= 0.5 * window {
                    height[(j * nx + i) as usize] = weighted / covered;
                }
            }
        }
        Self {
            origin: stamp.origin,
            cell_m: stamp.cell_m,
            size: stamp.size,
            height,
            datum_y: stamp.datum_y,
        }
    }

    /// The canonical `y` every height is measured up from.
    pub(crate) fn datum_y(&self) -> f32 {
        self.datum_y
    }

    /// The pooled grid as the stores' raster walks it: the canonical `(x, z)` of cell `(0, 0)`'s
    /// low corner, the cell side (m), the cells across and along, and the row-major heights
    /// ([`OPEN_M`] over an opening). Unmirrored: a mirrored read negates canonical `x` first.
    pub(crate) fn grid(&self) -> (Vec2, f32, UVec2, &[f32]) {
        (self.origin, self.cell_m, self.size, &self.height)
    }

    /// The underside's height over the datum at canonical `(x, z)`, m: [`OPEN_M`] over an opening
    /// and off the stamp. A `mirrored` read is the reflected shoe's, at `(−x, z)` of this one. The
    /// point law the stores' raster walks incrementally (`store::Raster`).
    #[cfg(test)]
    pub(crate) fn height(&self, x: f32, z: f32, mirrored: bool) -> f32 {
        let x = if mirrored { -x } else { x };
        let local = (Vec2::new(x, z) - self.origin) / self.cell_m;
        if local.x < 0.0 || local.y < 0.0 {
            return OPEN_M;
        }
        let cell = local.as_uvec2();
        if cell.x >= self.size.x || cell.y >= self.size.y {
            return OPEN_M;
        }
        self.height[(cell.y * self.size.x + cell.x) as usize]
    }

    /// The canonical `(x, z)` bounds a read covers, low and high, `mirrored` as in [`Self::height`].
    pub(crate) fn bounds(&self, mirrored: bool) -> (Vec2, Vec2) {
        let high = self.origin + self.size.as_vec2() * self.cell_m;
        if mirrored {
            (
                Vec2::new(-high.x, self.origin.y),
                Vec2::new(-self.origin.x, high.y),
            )
        } else {
            (self.origin, high)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box from `low` to `high`, as twelve outward-wound triangles.
    fn cuboid(low: Vec3, high: Vec3, into: &mut Vec<[Vec3; 3]>) {
        let corner = |x: bool, y: bool, z: bool| {
            Vec3::new(
                if x { high.x } else { low.x },
                if y { high.y } else { low.y },
                if z { high.z } else { low.z },
            )
        };
        let quads = [
            [(0, 0, 0), (1, 0, 0), (1, 0, 1), (0, 0, 1)],
            [(0, 1, 0), (0, 1, 1), (1, 1, 1), (1, 1, 0)],
            [(0, 0, 0), (0, 1, 0), (1, 1, 0), (1, 0, 0)],
            [(0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)],
            [(0, 0, 0), (0, 0, 1), (0, 1, 1), (0, 1, 0)],
            [(1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1)],
        ];
        for quad in quads {
            let [a, b, c, d] = quad.map(|(x, y, z)| corner(x == 1, y == 1, z == 1));
            into.push([a, b, c]);
            into.push([a, c, d]);
        }
    }

    /// A synthetic shoe, fixture geometry: a plate 0.60 m across and 0.16 m along whose underside
    /// sits at `y = 0`, a grouser bar 3 cm deep under its front third, and a 10 cm square opening
    /// cut through it on the lateral `+x` half, away from the bar. Built as the plate's four
    /// pieces around the opening, so the opening is a real hole and not an absent face.
    fn grousered_shoe() -> Vec<[Vec3; 3]> {
        let mut triangles = Vec::new();
        let (plate_y, top) = (0.0, 0.02);
        // The plate, around the opening at x ∈ [0.10, 0.20], z ∈ [−0.08, −0.02].
        cuboid(
            Vec3::new(-0.30, plate_y, -0.08),
            Vec3::new(0.10, top, 0.08),
            &mut triangles,
        );
        cuboid(
            Vec3::new(0.20, plate_y, -0.08),
            Vec3::new(0.30, top, 0.08),
            &mut triangles,
        );
        cuboid(
            Vec3::new(0.10, plate_y, -0.02),
            Vec3::new(0.20, top, 0.08),
            &mut triangles,
        );
        // The grouser: the front third, protruding 3 cm toward the ground.
        cuboid(
            Vec3::new(-0.30, -0.03, 0.03),
            Vec3::new(0.30, plate_y, 0.08),
            &mut triangles,
        );
        triangles
    }

    /// THE BAKE'S CONTRACT: the stamp is the LOWER ENVELOPE, so the part of a shoe that protrudes
    /// toward the ground is its datum and prints deepest, the plate behind it sits higher by the
    /// grouser's depth, and an opening through the shoe stays uncovered rather than filling with
    /// the plate's top face. Each print law reads these three facts: a bake that kept the TOP
    /// surface would print the plate flat, one that filled openings would print a solid sole.
    #[test]
    fn a_grouser_prints_deepest_and_an_opening_stays_uncovered() {
        let stamp = bake(&grousered_shoe()).expect("the shoe covers cells");
        assert!(
            (stamp.datum_y + 0.03).abs() < 1e-6,
            "the datum is the grouser's face, not {}",
            stamp.datum_y
        );
        let at = |x: f32, z: f32| {
            let cell = stamp.cell_of(x, z).expect("on the stamp");
            stamp.height_at(cell.x, cell.y)
        };
        let grouser = at(0.0, 0.055).expect("the grouser is covered");
        let plate = at(-0.2, -0.05).expect("the plate is covered");
        assert!(
            grouser < 1e-3,
            "the grouser is the lowest surface: {grouser}"
        );
        assert!(
            (plate - 0.03).abs() < 1e-3,
            "the plate sits a grouser's depth above it: {plate}"
        );
        assert_eq!(at(0.15, -0.05), None, "the opening prints nothing");

        // Prefiltered to a near-store texel, the grouser still stands most of its depth below the
        // plate, and an opening wider than a texel stays open.
        let pooled = ClearanceStamp::pooled(&stamp, 0.047);
        let (grouser, plate) = (
            pooled.height(0.0, 0.055, false),
            pooled.height(-0.2, -0.05, false),
        );
        assert!(
            plate - grouser > 0.75 * 0.03,
            "pooled: the grouser {grouser} m, the plate {plate} m",
        );
        assert!(pooled.height(0.15, -0.05, false) >= OPEN_M);
    }

    /// The left shoe reuses the right's stamp mirrored, which is right only when the mirror is the
    /// left mesh's own bake: reflecting the triangles across `x = 0` (winding reversed, as
    /// `link_view::mirrored_mesh` does) and baking must give the mirrored stamp, cell for cell.
    /// An asymmetric shoe, so a mirror that forgot to flip, or flipped about the wrong line, fails.
    #[test]
    fn the_mirrored_stamp_is_the_mirrored_shoes_bake() {
        let right = grousered_shoe();
        let left: Vec<[Vec3; 3]> = right
            .iter()
            .map(|&[a, b, c]| {
                let m = |v: Vec3| Vec3::new(-v.x, v.y, v.z);
                [m(a), m(c), m(b)]
            })
            .collect();
        let baked = bake(&left).expect("covers");
        let mirrored = bake(&right).expect("covers").mirrored();
        assert_eq!(baked.size, mirrored.size);
        assert!((baked.origin - mirrored.origin).abs().max_element() < 1e-6);
        assert_eq!(baked.coverage, mirrored.coverage);
        assert_eq!(baked.height, mirrored.height);

        let pooled = ClearanceStamp::pooled(&bake(&right).expect("covers"), 0.047);
        assert!(
            pooled.height(-0.15, -0.05, true) >= OPEN_M
                && pooled.height(0.15, -0.05, true) < OPEN_M,
            "the mirrored read finds the opening on the other side",
        );
    }
}
