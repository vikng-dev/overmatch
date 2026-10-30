//! The prints' stores (cosmetic terrain marks, view only), all CPU-side: the HISTORY every store is
//! read from, the near store's GPU copy around the camera, and the world layer over the whole map.
//! Each GPU texture is written tile by tile ([`Upload`]); nothing is ever read back.
//!
//! # A stamp, at one point of ground
//!
//! A [`StampEntry`] places a shoe's pooled underside ([`ClearanceStamp`]) on the ground: its datum
//! point and heading in world XZ, the plane its underside makes with the ground under it (a lift at
//! the canonical origin and a tilt across and along the shoe), how far into its press the link is,
//! and the ground pressure of the tank that laid it. The plane is normalised at the stamper so a
//! grounded shoe touches at its lowest corner (`super::stamper`).
//!
//! Where the plane lies within [`PrintLaw::contact_m`] of the ground the sole rests there: that is
//! the FOOTPRINT. Inside it the shoe's clearance `c` (the underside's height over its datum plus the
//! plane) sets the PATTERN, `1 − c / pattern_range`, so a grouser prints deeper than the plate
//! behind it and the whole footprint prints on every soft ground. The press scales the pattern from
//! nothing at touchdown to all of it one pitch of travel later. A texel stores two bytes:
//!
//! * R — pattern × press, in 1/255: the share of the ground's print depth this point sinks by;
//! * G — the tank's ground pressure, in [`KPA_PER_UNIT`]: zero off every footprint.
//!
//! Both combine by MAX, which is order-free, so any set of stamps draws the same texels in any
//! order and in any pieces, and a print only ever deepens. The shader turns a texel into a depth:
//! the ground's `softness × pressure`, capped, times R (`terrain_blend::print_depth_m`).
//!
//! # The history
//!
//! [`StampLog`]: the global near-resolution raster ([`NEAR_TEXEL_M`]), kept in [`CELL`]² cells. A
//! stamp is rasterised ONCE, into the history; the near store and the world layer are fed from the
//! texels it changed. Hot cells stay raw in a bounded cache; cold cells are run-length coded when
//! they leave it. Memory grows with visited area, never with repeat visits.
//!
//! # The near store
//!
//! [`NEAR_SIDE`]² texels on the GPU, TOROIDAL: global texel `g` lives at storage `g mod side`. The
//! window it holds follows the eye in whole cells ([`NearStore::recentre`]), so a history cell is
//! always one contiguous square of the GPU copy and the near store keeps no CPU texels of its own:
//! its uploads are read straight out of the history. A recentre uploads each cell that entered
//! (and clears a stale square only where the GPU still holds old marks), so the GPU copy is always
//! exactly the history over the window.
//!
//! # The world layer
//!
//! [`WORLD_SIDE`]² texels over the map's square. A world texel counts the near texels inside it
//! that a footprint has reached, and its GPU copy carries that count over the texel's near-texel
//! total, with a full mip chain the CPU builds tile by tile. Union, not sum: a near texel enters the
//! count once, the frame its G channel leaves zero, so a track laid over and over stays one track.

use bevy::math::{IVec2, UVec2, Vec2};
use bevy::platform::collections::HashMap;

use super::stamp::{ClearanceStamp, OPEN_M};

/// Texels across the near store. DERIVED from the approaches note's §4 starting point.
pub(crate) const NEAR_SIDE: u32 = 2048;
/// Metres across the near store. ASSUMED (approaches note §4): about twice the camera's farthest
/// orbit plus the distance the normal survives.
pub(crate) const NEAR_SPAN_M: f32 = 96.0;
/// The near store's texel, m: DERIVED, 4.6875 cm.
pub(crate) const NEAR_TEXEL_M: f32 = NEAR_SPAN_M / NEAR_SIDE as f32;
/// Near texels per history cell side, and the step the near window moves by: the window is
/// always `NEAR_SIDE / CELL` whole cells across. ASSUMED: 6 m, a strip small enough to upload in a
/// frame.
pub(crate) const CELL: i32 = 128;
/// How far the eye may wander from the window's centre, in texels along either axis, before the
/// window recentres. Above the step, so an eye on a step boundary does not flap. ASSUMED: 9 m,
/// which leaves 39 m of the window ahead of the eye in every direction.
const RECENTRE_SLACK: i32 = 192;
/// Texels across the world layer. DERIVED from the approaches note's §4 starting point: 0.37 m
/// on the shipped 1500 m map.
pub(crate) const WORLD_SIDE: u32 = 4096;
/// The near store's upload tile, texels: a cell goes to the GPU as `2 × 2` of them.
const NEAR_TILE: i32 = 64;
/// Near upload tiles per cell side.
const TILES_PER_CELL: i32 = CELL / NEAR_TILE;
/// World upload/rebuild tile side. ASSUMED: stamps touch only a few world texels, so rebuilding
/// larger squares up the mip chain per touched tile would dominate a pivot's cost.
const WORLD_UPLOAD_TILE: u32 = 8;
/// World count-tile side: counts are allocated in squares of this many texels, only where printed.
const WORLD_COUNT_TILE: u32 = 64;
/// Raw history cells kept hot. ASSUMED cache size; DERIVED raw payload cap 8 MiB. Cold cells remain
/// exact compressed checkpoints, never evicted history.
const ACTIVE_CELLS: usize = 256;
/// Ground pressure per unit of a texel's G channel, kPa: 1, so a byte spans 1–255 kPa, every tank.
pub(crate) const KPA_PER_UNIT: f32 = 1.0;

/// How stamps turn into store texels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PrintLaw {
    /// The clearance over which the pattern falls from full to none, m: a point of the sole that
    /// clears the ground by `c` prints `1 − c / pattern_range_m` of the ground's depth.
    pub(crate) pattern_range_m: f32,
    /// How far a sole's plane may clear the ground and still lie on it, m: the footprint's edge,
    /// and the stamper's test for a grounded shoe.
    pub(crate) contact_m: f32,
}

/// One stamp, as the stamper hands it over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StampEntry {
    /// World XZ of the shoe's datum point: its canonical origin at the underside's lowest height.
    x: f32,
    z: f32,
    /// The heading of the shoe's canonical `+z` in world XZ, in 1/65536 turns from `+Z` toward
    /// `+X`.
    yaw: u16,
    /// The underside plane's clearance at the canonical origin, 1/4096 m.
    lift: i16,
    /// The plane's slope across (`x`) and along (`z`) the shoe, 1/64 m per m.
    tilt: [i8; 2],
    /// Bit 0: the shoe is the mirror of its stamp (the left track).
    flags: u8,
    /// Which shoe stamp: an index into the stamps the stores are drawn with.
    shoe: u8,
    /// How far into its press the link is, 1/255: the share of its pattern it prints.
    press: u8,
    /// The ground pressure of the tank that laid it, in [`KPA_PER_UNIT`].
    pressure: u8,
}

const YAW_UNITS: f32 = 65536.0 / std::f32::consts::TAU;
const LIFT_UNITS: f32 = 4096.0;
// DERIVED: the range must include sin(60°), the largest shoe tilt accepted by the
// stamper on flat ground, plus the ground's own slope; 1/64 spans almost ±2 m/m.
const TILT_UNITS: f32 = 64.0;
const MIRRORED: u8 = 1;

/// The canonical placement a stamp is drawn with: datum, heading, underside plane, shoe.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    /// World XZ of the datum point.
    pub(crate) datum: Vec2,
    /// Heading of the canonical `+z`, rad from `+Z` toward `+X`.
    pub(crate) yaw: f32,
    /// The underside plane `lift + tilt · (x, z)` over the ground, in canonical `(x, z)`.
    pub(crate) lift: f32,
    pub(crate) tilt: Vec2,
    /// Which shoe stamp, and whether it is read mirrored.
    pub(crate) shoe: u8,
    pub(crate) mirrored: bool,
}

impl StampEntry {
    /// A stamp of `placement`, `press` (0..1) of the way into its press, laid by a tank whose ground
    /// pressure is `pressure_kpa`.
    pub(crate) fn new(placement: Placement, press: f32, pressure_kpa: f32) -> Self {
        let tilt = placement.tilt * TILT_UNITS;
        Self {
            x: placement.datum.x,
            z: placement.datum.y,
            // A turn is 65536 units, so wrapping the integer to 16 bits wraps the heading.
            yaw: (placement.yaw * YAW_UNITS).round() as i32 as u16,
            lift: (placement.lift * LIFT_UNITS)
                .round()
                .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16,
            tilt: [tilt.x, tilt.y].map(|t| t.round().clamp(-127.0, 127.0) as i8),
            flags: if placement.mirrored { MIRRORED } else { 0 },
            shoe: placement.shoe,
            press: (press.clamp(0.0, 1.0) * 255.0).round() as u8,
            pressure: (pressure_kpa / KPA_PER_UNIT).round().clamp(1.0, 255.0) as u8,
        }
    }

    pub(crate) fn datum(&self) -> Vec2 {
        Vec2::new(self.x, self.z)
    }

    pub(crate) fn mirrored(&self) -> bool {
        self.flags & MIRRORED != 0
    }

    pub(crate) fn shoe(&self) -> usize {
        usize::from(self.shoe)
    }

    /// The share of its pattern this stamp prints, 0..1.
    pub(crate) fn press(&self) -> f32 {
        f32::from(self.press) / 255.0
    }

    /// The shoe's canonical `x` and `z` axes in world XZ.
    fn axes(&self) -> (Vec2, Vec2) {
        let (sin, cos) = (f32::from(self.yaw) / YAW_UNITS).sin_cos();
        (Vec2::new(cos, -sin), Vec2::new(sin, cos))
    }

    /// The underside plane's clearance at canonical `c`, m.
    fn plane(&self, c: Vec2) -> f32 {
        f32::from(self.lift) / LIFT_UNITS
            + (f32::from(self.tilt[0]) * c.x + f32::from(self.tilt[1]) * c.y) / TILT_UNITS
    }

    /// The world XZ box the shoe's stamp covers: low, high.
    #[cfg(test)]
    pub(crate) fn world_bounds(&self, shoe: &ClearanceStamp) -> (Vec2, Vec2) {
        self.world_bounds_along(shoe, self.axes())
    }

    /// The world XZ box the shoe's stamp covers, low and high, with its axes already in hand.
    fn world_bounds_along(
        &self,
        shoe: &ClearanceStamp,
        (across, along): (Vec2, Vec2),
    ) -> (Vec2, Vec2) {
        let (low, high) = shoe.bounds(self.mirrored());
        [
            Vec2::new(low.x, low.y),
            Vec2::new(high.x, low.y),
            Vec2::new(low.x, high.y),
            Vec2::new(high.x, high.y),
        ]
        .into_iter()
        .map(|c| self.datum() + across * c.x + along * c.y)
        .fold(
            (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)),
            |(lo, hi), p| (lo.min(p), hi.max(p)),
        )
    }

    /// The texel this stamp writes at world XZ `world`, `[R, G]`; `None` off the footprint, over an
    /// opening, and where the pattern prints nothing. The raster's law at one point, which
    /// [`Raster`] evaluates incrementally.
    #[cfg(test)]
    fn texel_at(&self, shoe: &ClearanceStamp, world: Vec2, law: &PrintLaw) -> Option<[u8; 2]> {
        let (across, along) = self.axes();
        let d = world - self.datum();
        let c = Vec2::new(d.dot(across), d.dot(along));
        let height = shoe.height(c.x, c.y, self.mirrored());
        let plane = self.plane(c);
        if height >= OPEN_M || plane > law.contact_m {
            return None;
        }
        let pattern = (1.0 - (height + plane) / law.pattern_range_m).clamp(0.0, 1.0);
        let r = (self.press() * pattern * 255.0).round() as u8;
        (r > 0).then_some([r, self.pressure])
    }
}

/// A half-open texel rectangle, `min` included, `max` excluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) min: IVec2,
    pub(crate) max: IVec2,
}

impl Span {
    fn intersect(self, other: Self) -> Option<Self> {
        let span = Self {
            min: self.min.max(other.min),
            max: self.max.min(other.max),
        };
        (span.min.x < span.max.x && span.min.y < span.max.y).then_some(span)
    }

    /// The texels of `texel_m` whose centres a world box from `low` to `high` holds.
    fn covering(low: Vec2, high: Vec2, texel_m: f32) -> Self {
        // Scalar on purpose: the vector `ceil` and `floor` go through `libm` in this build.
        let first = |at: f32| (at / texel_m - 0.5).ceil() as i32;
        let end = |at: f32| (at / texel_m - 0.5).floor() as i32 + 1;
        Self {
            min: IVec2::new(first(low.x), first(low.y)),
            max: IVec2::new(end(high.x), end(high.y)),
        }
    }

    fn contains(&self, g: IVec2) -> bool {
        g.x >= self.min.x && g.x < self.max.x && g.y >= self.min.y && g.y < self.max.y
    }
}

/// One stamp laid on the global near raster: every per-texel quantity is an affine function of the
/// texel's index relative to `base`, so the raster walks it by additions. `u`, `v` index the pooled
/// grid in cells (the mirror folded in), `plane` is the underside plane's clearance, each as
/// `[at base, per column, per row]`.
struct Raster<'a> {
    heights: &'a [f32],
    width: i32,
    height: i32,
    span: Span,
    base: IVec2,
    u: [f32; 3],
    v: [f32; 3],
    plane: [f32; 3],
    /// Per pooled-grid axis, the columns of row `j` inside it, `[lo, lo per row, hi, hi per row]`
    /// (inclusive, padded by [`ROW_PAD`]); `None` where the axis is constant along a row, which
    /// is checked per row instead.
    strips: [Option<[f32; 4]>; 2],
    /// The press in 1/255, and how much of it a metre of clearance takes away.
    full: f32,
    per_metre: f32,
    pressure: u8,
    contact_m: f32,
}

/// How far, in columns, a row's analytic interval is widened, so a rounding at its ends never drops
/// a texel; the per-texel test decides. DERIVED: far above the `f32` error of the interval (about
/// 1e-6 of a column at the stamp's extent), far below a column.
const ROW_PAD: f32 = 1e-3;

impl<'a> Raster<'a> {
    fn new(entry: &StampEntry, shoe: &'a ClearanceStamp, law: &PrintLaw) -> Option<Self> {
        if entry.press == 0 {
            return None;
        }
        let axes = entry.axes();
        let (low, high) = entry.world_bounds_along(shoe, axes);
        let span = Span::covering(low, high, NEAR_TEXEL_M);
        if span.min.x >= span.max.x || span.min.y >= span.max.y {
            return None;
        }
        let (origin, cell_m, size, heights) = shoe.grid();
        let (across, along) = axes;
        let base = span.min;
        // Canonical coordinates of texel `base + (i, j)`'s centre: `c0 + i·ci + j·cj`.
        let d0 = (base.as_vec2() + 0.5) * NEAR_TEXEL_M - entry.datum();
        let c0 = Vec2::new(d0.dot(across), d0.dot(along));
        let ci = Vec2::new(across.x, along.x) * NEAR_TEXEL_M;
        let cj = Vec2::new(across.y, along.y) * NEAR_TEXEL_M;
        let mirror = if entry.mirrored() { -1.0 } else { 1.0 };
        let u = [
            (mirror * c0.x - origin.x) / cell_m,
            mirror * ci.x / cell_m,
            mirror * cj.x / cell_m,
        ];
        let v = [(c0.y - origin.y) / cell_m, ci.y / cell_m, cj.y / cell_m];
        // Along a row, `0 ≤ a0 + a2·j + a1·i < extent` holds for `i` between two bounds, each
        // linear in `j`.
        let strip = |a: [f32; 3], extent: f32| {
            (a[1].abs() > 1e-6).then(|| {
                let r = 1.0 / a[1];
                let (b0, b1, per_row) = (-a[0] * r, (extent - a[0]) * r, -a[2] * r);
                let (lo, hi) = if r > 0.0 { (b0, b1) } else { (b1, b0) };
                [lo - ROW_PAD, per_row, hi + ROW_PAD, per_row]
            })
        };
        let full = entry.press() * 255.0;
        let tilt = |c: Vec2| entry.plane(c) - entry.plane(Vec2::ZERO);
        Some(Self {
            heights,
            width: size.x as i32,
            height: size.y as i32,
            span,
            base,
            strips: [strip(u, size.x as f32), strip(v, size.y as f32)],
            u,
            v,
            plane: [entry.plane(c0), tilt(ci), tilt(cj)],
            full,
            per_metre: full / law.pattern_range_m,
            pressure: entry.pressure,
            contact_m: law.contact_m,
        })
    }

    /// Merge this stamp into `texels`, the history cell whose low corner is global texel `min`,
    /// over `clip` (inside that cell). Returns the cell's changed upload tiles as bits, and counts
    /// each texel whose footprint channel left zero into `world`.
    ///
    /// Written as plain scalar loops over locals: this is the prints' hot loop, and the edit
    /// profile builds the crate at `opt-level = 1`, where iterator adapters and closures are not
    /// flattened away.
    fn draw(&self, texels: &mut [[u8; 2]], min: IVec2, clip: Span, world: &mut WorldStore) -> u8 {
        let [u0, u1, u2] = self.u;
        let [v0, v1, v2] = self.v;
        let [p0, p1, p2] = self.plane;
        let (width, height) = (self.width, self.height);
        let (wf, hf) = (width as f32, height as f32);
        let (full, per_metre, contact_m, pressure) =
            (self.full, self.per_metre, self.contact_m, self.pressure);
        let heights = self.heights;
        let mut changed = 0_u8;
        for gj in clip.min.y..clip.max.y {
            let j = gj - self.base.y;
            let jf = j as f32;
            // The row's columns on the pooled grid: inside both axes' strips.
            let mut lo = (clip.min.x - self.base.x) as f32;
            let mut hi = (clip.max.x - self.base.x - 1) as f32;
            match self.strips[0] {
                Some([lo0, lo_j, hi0, hi_j]) => {
                    lo = lo.max(lo0 + lo_j * jf);
                    hi = hi.min(hi0 + hi_j * jf);
                }
                None => {
                    let at = u0 + u2 * jf;
                    if !(at >= 0.0 && at < wf) {
                        continue;
                    }
                }
            }
            match self.strips[1] {
                Some([lo0, lo_j, hi0, hi_j]) => {
                    lo = lo.max(lo0 + lo_j * jf);
                    hi = hi.min(hi0 + hi_j * jf);
                }
                None => {
                    let at = v0 + v2 * jf;
                    if !(at >= 0.0 && at < hf) {
                        continue;
                    }
                }
            }
            if hi < lo {
                continue;
            }
            let (first, end) = (lo.ceil() as i32, hi.floor() as i32 + 1);
            let (u_row, v_row, p_row) = (u0 + u2 * jf, v0 + v2 * jf, p0 + p2 * jf);
            let local_j = gj - min.y;
            // Global column `base.x + i` is local column `i + shift`.
            let shift = self.base.x - min.x;
            let row = local_j * CELL + shift;
            let (mut first_changed, mut last_changed) = (i32::MAX, i32::MIN);
            for i in first..end {
                let fi = i as f32;
                let (u, v) = (u_row + u1 * fi, v_row + v1 * fi);
                if u < 0.0 || v < 0.0 || u >= wf || v >= hf {
                    continue;
                }
                let lift = heights[(v as i32 * width + u as i32) as usize];
                let plane = p_row + p1 * fi;
                if lift >= OPEN_M || plane > contact_m {
                    continue;
                }
                // The pattern, `1 − clearance / range`, times the press, in 1/255.
                let r = (full - (lift + plane) * per_metre + 0.5).clamp(0.0, 255.0) as u8;
                if r == 0 {
                    continue;
                }
                let texel = &mut texels[(row + i) as usize];
                let [old_r, old_g] = *texel;
                if r <= old_r && pressure <= old_g {
                    continue;
                }
                *texel = [old_r.max(r), old_g.max(pressure)];
                first_changed = first_changed.min(i);
                last_changed = i;
                if old_g == 0 {
                    world.reach(IVec2::new(i + shift, local_j) + min);
                }
            }
            if first_changed <= last_changed {
                let tile_row = (local_j / NEAR_TILE) * TILES_PER_CELL;
                for tile in (first_changed + shift) / NEAR_TILE..=(last_changed + shift) / NEAR_TILE
                {
                    changed |= 1 << (tile_row + tile);
                }
            }
        }
        changed
    }
}

/// Spatial stamp history, checkpointed on the global near raster ([module doc](self)).
#[derive(Default)]
pub(crate) struct StampLog {
    count: usize,
    cells: HashMap<IVec2, LogCell>,
    active: Vec<IVec2>,
}

#[derive(Default)]
struct LogCell {
    checkpoint: Checkpoint,
    last: usize,
    active: bool,
}

/// Exclusive end of a row-major run, and its value. DERIVED: a cell has 16384 texels,
/// so an end fits in u16. Raw storage bounds incompressible checkpoints to 32 KiB.
struct Run {
    end: u16,
    value: [u8; 2],
}

#[derive(Default)]
enum Checkpoint {
    #[default]
    Empty,
    Runs(Box<[Run]>),
    Raw(Box<[[u8; 2]]>),
}

impl Checkpoint {
    fn visit(&self, mut put: impl FnMut(usize, [u8; 2])) {
        match self {
            Self::Empty => {}
            Self::Raw(texels) => {
                for (i, &value) in texels.iter().enumerate() {
                    if value != [0, 0] {
                        put(i, value);
                    }
                }
            }
            Self::Runs(runs) => {
                let mut start = 0;
                for run in runs {
                    if run.value != [0, 0] {
                        for i in start..usize::from(run.end) {
                            put(i, run.value);
                        }
                    }
                    start = usize::from(run.end);
                }
            }
        }
    }

    fn raw(&mut self) -> &mut [[u8; 2]] {
        if !matches!(self, Self::Raw(_)) {
            let mut texels = vec![[0, 0]; (CELL * CELL) as usize];
            self.visit(|i, value| texels[i] = value);
            *self = Self::Raw(texels.into_boxed_slice());
        }
        let Self::Raw(texels) = self else {
            unreachable!("converted to raw");
        };
        texels
    }

    fn packed(texels: Vec<[u8; 2]>) -> Self {
        if texels.iter().all(|&value| value == [0, 0]) {
            return Self::Empty;
        }
        let mut runs: Vec<Run> = Vec::new();
        for (i, &value) in texels.iter().enumerate() {
            if let Some(last) = runs.last_mut()
                && last.value == value
            {
                last.end = (i + 1) as u16;
            } else {
                runs.push(Run {
                    end: (i + 1) as u16,
                    value,
                });
            }
        }
        if runs.len() * std::mem::size_of::<Run>() >= texels.len() * 2 {
            Self::Raw(texels.into_boxed_slice())
        } else {
            Self::Runs(runs.into_boxed_slice())
        }
    }
}

impl StampLog {
    /// The raw texels of cell `key`, made hot. Compression happens only on cache turnover, so a
    /// pivot over the same ground never recompresses a cell it keeps writing.
    fn hot(&mut self, key: IVec2) -> &mut LogCell {
        if !self.cells.get(&key).is_some_and(|cell| cell.active) {
            if self.active.len() == ACTIVE_CELLS {
                let oldest = (0..self.active.len())
                    .min_by_key(|&i| self.cells[&self.active[i]].last)
                    .expect("a full cache");
                let cold = self.active.swap_remove(oldest);
                let cell = self.cells.get_mut(&cold).expect("active history cell");
                if let Checkpoint::Raw(texels) = std::mem::take(&mut cell.checkpoint) {
                    cell.checkpoint = Checkpoint::packed(texels.into_vec());
                }
                cell.active = false;
            }
            let cell = self.cells.entry(key).or_default();
            cell.checkpoint.raw();
            cell.active = true;
            self.active.push(key);
        }
        let cell = self.cells.get_mut(&key).expect("a hot history cell");
        cell.last = self.count;
        cell
    }

    /// Copy cell `key`'s texels into `out` (row-major, [`CELL`]²). `false`, with `out` untouched,
    /// where nothing was ever printed.
    fn copy_cell(&self, key: IVec2, out: &mut [[u8; 2]]) -> bool {
        let Some(cell) = self.cells.get(&key) else {
            return false;
        };
        match &cell.checkpoint {
            Checkpoint::Empty => return false,
            Checkpoint::Raw(texels) => out.copy_from_slice(texels),
            checkpoint => {
                out.fill([0, 0]);
                checkpoint.visit(|i, value| out[i] = value);
            }
        }
        true
    }

    /// The historical texels in `span`, independent of cache residency.
    #[cfg(test)]
    fn draw(&self, span: Span, mut put: impl FnMut(IVec2, [u8; 2])) {
        let low = span.min.div_euclid(IVec2::splat(CELL));
        let high = (span.max - 1).div_euclid(IVec2::splat(CELL));
        for j in low.y..=high.y {
            for i in low.x..=high.x {
                let key = IVec2::new(i, j);
                let Some(cell) = self.cells.get(&key) else {
                    continue;
                };
                let min = key * CELL;
                cell.checkpoint.visit(|index, value| {
                    let g = min + IVec2::new(index as i32 % CELL, index as i32 / CELL);
                    if span.contains(g) {
                        put(g, value);
                    }
                });
            }
        }
    }

    /// Stamps laid so far.
    pub(crate) fn len(&self) -> usize {
        self.count
    }
}

/// Which store an upload writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Store {
    Near,
    World,
}

impl Store {
    pub(crate) fn bytes_per_texel(self) -> u32 {
        match self {
            Store::Near => 2,
            Store::World => 1,
        }
    }
}

/// One rectangle of one mip level of a store's GPU copy, its texels row by row.
#[derive(Clone, Debug)]
pub(crate) struct Upload {
    pub(crate) store: Store,
    pub(crate) level: u32,
    pub(crate) origin: UVec2,
    pub(crate) size: UVec2,
    pub(crate) data: Vec<u8>,
}

/// The near store ([module doc](self)): which cells the GPU copy holds, which of their tiles wait
/// for an upload, and where the GPU copy may still hold marks.
pub(crate) struct NearStore {
    /// The window's low corner in global texels, a multiple of [`CELL`]; `None` before the first
    /// placement.
    origin: Option<IVec2>,
    /// History cells in the window whose upload tiles changed, as tile bits.
    dirty: HashMap<IVec2, u8>,
    /// Per storage upload tile: whether the GPU copy may hold a nonzero texel there. Zero-initialised
    /// by wgpu, so all false at startup.
    gpu_nonzero: Vec<bool>,
}

impl Default for NearStore {
    fn default() -> Self {
        let tiles = (NEAR_SIDE as i32 / NEAR_TILE) as usize;
        Self {
            origin: None,
            dirty: HashMap::default(),
            gpu_nonzero: vec![false; tiles * tiles],
        }
    }
}

impl NearStore {
    /// The global texels the store holds; `None` before the first placement.
    pub(crate) fn window(&self) -> Option<Span> {
        self.origin.map(|min| Span {
            min,
            max: min + NEAR_SIDE as i32,
        })
    }

    /// The window's low corner, world XZ m.
    pub(crate) fn origin_m(&self) -> Option<Vec2> {
        self.origin.map(|origin| origin.as_vec2() * NEAR_TEXEL_M)
    }

    fn holds(&self, key: IVec2) -> bool {
        self.window()
            .is_some_and(|window| window.contains(key * CELL))
    }

    /// Note that cell `key`'s tiles `bits` changed.
    fn mark(&mut self, key: IVec2, bits: u8) {
        if bits != 0 && self.holds(key) {
            *self.dirty.entry(key).or_default() |= bits;
        }
    }

    /// Follow the eye at world XZ `eye`: once it has wandered more than [`RECENTRE_SLACK`] from the
    /// window's centre (or on the first call), move the window onto it in whole cells, and queue
    /// every cell that entered. Whether the window moved.
    fn recentre(&mut self, eye: Vec2) -> bool {
        let half = NEAR_SIDE as i32 / 2;
        let eye = (eye / NEAR_TEXEL_M).floor().as_ivec2();
        if let Some(origin) = self.origin
            && (eye - (origin + half)).abs().max_element() <= RECENTRE_SLACK
        {
            return false;
        }
        let old = self.window();
        let origin = (eye - half).div_euclid(IVec2::splat(CELL)) * CELL;
        self.origin = Some(origin);
        let cells = NEAR_SIDE as i32 / CELL;
        let all = (1 << (TILES_PER_CELL * TILES_PER_CELL)) - 1;
        for j in 0..cells {
            for i in 0..cells {
                let key = origin / CELL + IVec2::new(i, j);
                if !old.is_some_and(|old| old.contains(key * CELL)) {
                    self.dirty.insert(key, all);
                }
            }
        }
        true
    }

    /// The tiles written since the last call, read out of `log`, as uploads of the GPU copy's
    /// single level. A tile whose history is empty is sent only where the GPU may still hold an old
    /// mark there, as zeros.
    fn take_uploads(&mut self, log: &StampLog) -> Vec<Upload> {
        let mut out = Vec::new();
        if self.dirty.is_empty() {
            return out;
        }
        let mut cell = vec![[0_u8; 2]; (CELL * CELL) as usize];
        let tiles = NEAR_SIDE as i32 / NEAR_TILE;
        for (key, bits) in std::mem::take(&mut self.dirty) {
            if !self.holds(key) {
                continue;
            }
            if !log.copy_cell(key, &mut cell) {
                cell.fill([0, 0]);
            }
            for tile in 0..TILES_PER_CELL * TILES_PER_CELL {
                if bits & (1 << tile) == 0 {
                    continue;
                }
                let at = IVec2::new(tile % TILES_PER_CELL, tile / TILES_PER_CELL) * NEAR_TILE;
                let storage = (key * CELL + at).rem_euclid(IVec2::splat(NEAR_SIDE as i32));
                let slot = ((storage.y / NEAR_TILE) * tiles + storage.x / NEAR_TILE) as usize;
                let mut data = Vec::with_capacity((NEAR_TILE * NEAR_TILE * 2) as usize);
                for row in at.y..at.y + NEAR_TILE {
                    let start = (row * CELL + at.x) as usize;
                    for texel in &cell[start..start + NEAR_TILE as usize] {
                        data.extend_from_slice(texel);
                    }
                }
                let nonzero = data.iter().any(|&b| b != 0);
                if !nonzero && !self.gpu_nonzero[slot] {
                    continue;
                }
                self.gpu_nonzero[slot] = nonzero;
                out.push(Upload {
                    store: Store::Near,
                    level: 0,
                    origin: storage.as_uvec2(),
                    size: UVec2::splat(NEAR_TILE as u32),
                    data,
                });
            }
        }
        out
    }
}

/// The world layer ([module doc](self)).
pub(crate) struct WorldStore {
    side: u32,
    /// The world texel (along either axis) the near texel `near_min + k` falls in, or `u16::MAX`
    /// off the map. The map is a square centred on the origin, so one table serves both axes.
    near_to_world: Vec<u16>,
    near_min: i32,
    /// Near texel centres inside world texel `k` along an axis: a world texel holds
    /// `per_axis[i] × per_axis[j]` of them.
    per_axis: Vec<u8>,
    /// Level 0: footprint near texels counted per world texel, in [`WORLD_COUNT_TILE`]² tiles
    /// allocated only where printed.
    counts: Vec<Option<Box<[u8]>>>,
    /// Levels 1 and up, coverage in 1/255: `mips[k − 1]` is level `k`.
    mips: Vec<Vec<u8>>,
    /// Level-0 upload tiles written since the last [`Self::take_uploads`]: a flag per tile, and the
    /// flagged tiles as a list, so a frame's work is the tiles it touched and never the whole map.
    dirty: Vec<bool>,
    dirty_tiles: Vec<UVec2>,
}

impl WorldStore {
    /// A world layer of `side`² texels over the square of `size_m` centred on the origin.
    pub(crate) fn new(size_m: f32, side: u32) -> Self {
        let texel_m = f64::from(size_m) / f64::from(side);
        let origin_m = -0.5 * f64::from(size_m);
        let near_m = f64::from(NEAR_TEXEL_M);
        let near_min = (origin_m / near_m).floor() as i32 - 1;
        let near_max = (-origin_m / near_m).ceil() as i32 + 1;
        let mut per_axis = vec![0_u8; side as usize];
        let near_to_world = (near_min..near_max)
            .map(|g| {
                let w = (((f64::from(g) + 0.5) * near_m - origin_m) / texel_m).floor();
                if (0.0..f64::from(side)).contains(&w) {
                    per_axis[w as usize] += 1;
                    w as u16
                } else {
                    u16::MAX
                }
            })
            .collect();
        let count_tiles = side.div_ceil(WORLD_COUNT_TILE) as usize;
        let upload_tiles = side.div_ceil(WORLD_UPLOAD_TILE) as usize;
        let mips = (1..Self::levels_of(side))
            .map(|k| vec![0; ((side >> k) * (side >> k)) as usize])
            .collect();
        Self {
            side,
            near_to_world,
            near_min,
            per_axis,
            counts: vec![None; count_tiles * count_tiles],
            mips,
            dirty: vec![false; upload_tiles * upload_tiles],
            dirty_tiles: Vec::new(),
        }
    }

    fn levels_of(side: u32) -> u32 {
        side.ilog2() + 1
    }

    /// The GPU copy's mip levels.
    pub(crate) fn levels(&self) -> u32 {
        Self::levels_of(self.side)
    }

    /// The world texel holding near texel `g`'s centre, if it is on the map.
    fn world_of(&self, g: IVec2) -> Option<UVec2> {
        let at = |g: i32| {
            self.near_to_world
                .get(usize::try_from(g - self.near_min).ok()?)
                .copied()
                .filter(|&w| w != u16::MAX)
        };
        Some(UVec2::new(u32::from(at(g.x)?), u32::from(at(g.y)?)))
    }

    /// Near texel `g` became footprint: count it into its world texel.
    fn reach(&mut self, g: IVec2) {
        let Some(w) = self.world_of(g) else {
            return;
        };
        let tiles = self.side.div_ceil(WORLD_COUNT_TILE);
        let tile = self.counts
            [((w.y / WORLD_COUNT_TILE) * tiles + w.x / WORLD_COUNT_TILE) as usize]
            .get_or_insert_with(|| {
                vec![0; (WORLD_COUNT_TILE * WORLD_COUNT_TILE) as usize].into_boxed_slice()
            });
        let count = &mut tile
            [((w.y % WORLD_COUNT_TILE) * WORLD_COUNT_TILE + w.x % WORLD_COUNT_TILE) as usize];
        *count = count.saturating_add(1);
        let upload_tiles = self.side.div_ceil(WORLD_UPLOAD_TILE);
        let tile = w / WORLD_UPLOAD_TILE;
        let flag = &mut self.dirty[(tile.y * upload_tiles + tile.x) as usize];
        if !std::mem::replace(flag, true) {
            self.dirty_tiles.push(tile);
        }
    }

    /// Coverage of texel `(i, j)` at mip `level`, in 1/255.
    pub(crate) fn coverage(&self, level: u32, i: u32, j: u32) -> u8 {
        if level == 0 {
            let tiles = self.side.div_ceil(WORLD_COUNT_TILE);
            let count = self.counts
                [((j / WORLD_COUNT_TILE) * tiles + i / WORLD_COUNT_TILE) as usize]
                .as_ref()
                .map_or(0, |tile| {
                    tile[((j % WORLD_COUNT_TILE) * WORLD_COUNT_TILE + i % WORLD_COUNT_TILE)
                        as usize]
                });
            let total = u32::from(self.per_axis[i as usize]) * u32::from(self.per_axis[j as usize]);
            if total == 0 {
                return 0;
            }
            ((u32::from(count) * 255 + total / 2) / total).min(255) as u8
        } else {
            self.mips[level as usize - 1][(j * (self.side >> level) + i) as usize]
        }
    }

    /// The level-0 tiles written since the last call and every mip tile above them, rebuilt and as
    /// uploads, finest level first.
    fn take_uploads(&mut self) -> Vec<Upload> {
        let tiles = self.side.div_ceil(WORLD_UPLOAD_TILE);
        let mut dirty = std::mem::take(&mut self.dirty_tiles);
        for t in &dirty {
            self.dirty[(t.y * tiles + t.x) as usize] = false;
        }
        dirty.sort_unstable_by_key(|t| (t.y, t.x));
        let mut out = Vec::new();
        for level in 0..self.levels() {
            let side = self.side >> level;
            let tile = WORLD_UPLOAD_TILE.min(side);
            if level > 0 {
                for t in &mut dirty {
                    *t /= 2;
                }
                dirty.sort_unstable_by_key(|t| (t.y, t.x));
                dirty.dedup();
                for &t in &dirty {
                    self.rebuild(level, t * tile, tile);
                }
            }
            for &t in &dirty {
                let origin = t * tile;
                let mut data = Vec::with_capacity((tile * tile) as usize);
                for j in origin.y..origin.y + tile {
                    for i in origin.x..origin.x + tile {
                        data.push(self.coverage(level, i, j));
                    }
                }
                out.push(Upload {
                    store: Store::World,
                    level,
                    origin,
                    size: UVec2::splat(tile),
                    data,
                });
            }
        }
        out
    }

    /// Rebuild `level`'s square of `tile` texels at `origin` from the level below: each texel the
    /// rounded mean of its four.
    fn rebuild(&mut self, level: u32, origin: UVec2, tile: u32) {
        let side = self.side >> level;
        for j in origin.y..origin.y + tile {
            for i in origin.x..origin.x + tile {
                let sum: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .into_iter()
                    .map(|(di, dj)| u32::from(self.coverage(level - 1, 2 * i + di, 2 * j + dj)))
                    .sum();
                self.mips[level as usize - 1][(j * side + i) as usize] = ((sum + 2) / 4) as u8;
            }
        }
    }
}

/// The three stores together: every stamp goes through [`Self::stamp`], every frame's GPU writes
/// come out of [`Self::take_uploads`].
pub(crate) struct PrintStores {
    pub(crate) log: StampLog,
    pub(crate) near: NearStore,
    pub(crate) world: WorldStore,
}

impl PrintStores {
    /// Empty stores for a map of side `world_size_m`, its world layer `world_side`² texels.
    pub(crate) fn new(world_size_m: f32, world_side: u32) -> Self {
        Self {
            log: StampLog::default(),
            near: NearStore::default(),
            world: WorldStore::new(world_size_m, world_side),
        }
    }

    /// Lay one stamp: rasterise it once into the history, and feed what it changed to the near
    /// store's uploads and the world layer's counts.
    pub(crate) fn stamp(&mut self, entry: &StampEntry, shoes: &[ClearanceStamp], law: &PrintLaw) {
        let Some(shoe) = shoes.get(entry.shoe()) else {
            return;
        };
        self.log.count += 1;
        let Some(raster) = Raster::new(entry, shoe, law) else {
            return;
        };
        let low = raster.span.min.div_euclid(IVec2::splat(CELL));
        let high = (raster.span.max - 1).div_euclid(IVec2::splat(CELL));
        for j in low.y..=high.y {
            for i in low.x..=high.x {
                let key = IVec2::new(i, j);
                let min = key * CELL;
                let Some(clip) = raster.span.intersect(Span {
                    min,
                    max: min + CELL,
                }) else {
                    continue;
                };
                let cell = self.log.hot(key);
                let texels = cell.checkpoint.raw();
                let changed = raster.draw(texels, min, clip, &mut self.world);
                self.near.mark(key, changed);
            }
        }
    }

    /// Keep the near window on the eye at world XZ `eye`. Whether it moved.
    pub(crate) fn recentre(&mut self, eye: Vec2) -> bool {
        self.near.recentre(eye)
    }

    /// Every GPU write since the last call: the near store's changed tiles, then the world layer's.
    pub(crate) fn take_uploads(&mut self) -> Vec<Upload> {
        let mut uploads = self.near.take_uploads(&self.log);
        uploads.extend(self.world.take_uploads());
        uploads
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track_prints::stamp::{ShoeStamp, bake};
    use bevy::math::Vec3;

    /// A plain sole, fixture geometry: 0.6 m across, 0.16 m along, flat underside.
    pub(crate) fn sole() -> ShoeStamp {
        let c = |x: f32, z: f32| Vec3::new(x, 0.0, z);
        let triangles = vec![
            [c(-0.3, -0.08), c(0.3, -0.08), c(0.3, 0.08)],
            [c(-0.3, -0.08), c(0.3, 0.08), c(-0.3, 0.08)],
        ];
        bake(&triangles).expect("covers")
    }

    const LAW: PrintLaw = PrintLaw {
        pattern_range_m: 0.10,
        contact_m: 0.03,
    };

    /// Fixture ground pressure, kPa.
    const KPA: f32 = 100.0;

    fn entry(datum: Vec2, yaw: f32, lift: f32, tilt: Vec2, mirrored: bool) -> StampEntry {
        StampEntry::new(
            Placement {
                datum,
                yaw,
                lift,
                tilt,
                shoe: 0,
                mirrored,
            },
            1.0,
            KPA,
        )
    }

    /// The GPU copies as the uploads leave them: what the shader would read.
    struct Gpu {
        near: Vec<[u8; 2]>,
        world: Vec<Vec<u8>>,
    }

    impl Gpu {
        fn new(world_side: u32) -> Self {
            Self {
                near: vec![[0, 0]; (NEAR_SIDE * NEAR_SIDE) as usize],
                world: (0..=world_side.ilog2())
                    .map(|k| vec![0; ((world_side >> k) * (world_side >> k)) as usize])
                    .collect(),
            }
        }

        fn apply(&mut self, uploads: &[Upload], world_side: u32) {
            for upload in uploads {
                for j in 0..upload.size.y {
                    for i in 0..upload.size.x {
                        let (x, y) = (upload.origin.x + i, upload.origin.y + j);
                        let k = (j * upload.size.x + i) as usize;
                        match upload.store {
                            Store::Near => {
                                self.near[(y * NEAR_SIDE + x) as usize] =
                                    [upload.data[2 * k], upload.data[2 * k + 1]];
                            }
                            Store::World => {
                                let side = world_side >> upload.level;
                                self.world[upload.level as usize][(y * side + x) as usize] =
                                    upload.data[k];
                            }
                        }
                    }
                }
            }
        }

        fn near_at(&self, g: IVec2) -> [u8; 2] {
            let s = g.rem_euclid(IVec2::splat(NEAR_SIDE as i32));
            self.near[(s.y as u32 * NEAR_SIDE + s.x as u32) as usize]
        }
    }

    /// Stores over a fixture map, with their GPU copies.
    fn stores() -> (PrintStores, Gpu) {
        (PrintStores::new(1500.0, WORLD_SIDE), Gpu::new(WORLD_SIDE))
    }

    fn flush(stores: &mut PrintStores, gpu: &mut Gpu) -> Vec<Upload> {
        let uploads = stores.take_uploads();
        gpu.apply(&uploads, WORLD_SIDE);
        uploads
    }

    /// The stores must retain the grounded edge of a tilted shoe. Clipping an accepted
    /// contact's slope while keeping its lift raises that edge and erases its footprint.
    #[test]
    fn encoded_tilt_preserves_an_accepted_edge_contact() {
        use bevy::math::{Affine3A, Quat};
        let shoe = ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M);
        let bounds = shoe.bounds(false);
        for degrees in [-55.0_f32, -45.0, 45.0, 55.0] {
            let angle = degrees.to_radians();
            let pose = Affine3A::from_rotation_translation(
                Quat::from_rotation_z(angle),
                Vec3::Y * bounds.1.x * angle.sin().abs(),
            );
            let contact = crate::track_prints::stamper::contact(
                pose,
                bounds,
                shoe.datum_y(),
                LAW.contact_m,
                |_| Some(0.0),
            )
            .expect("one edge rests on the ground");
            let entry = entry(
                contact.datum,
                contact.yaw,
                contact.lift,
                contact.tilt,
                false,
            );
            let edge = Vec2::new(-angle.signum() * (bounds.1.x - 0.01), 0.0);
            assert!(
                entry.plane(edge).abs() < LAW.contact_m,
                "the encoded {degrees} degree contact retains its grounded edge"
            );
            assert!(entry.texel_at(&shoe, edge, &LAW).is_some());
        }
    }

    /// THE RASTER IS THE POINT LAW. The incremental row walk must write exactly the texels the
    /// law gives at their centres, for every heading, mirror and tilt: a walk whose row interval
    /// clipped a texel, or whose increments drifted from the law, would print a shoe with ragged
    /// ends that the recentre test, comparing the store with itself, cannot see.
    #[test]
    fn the_row_walk_writes_what_the_point_law_gives() {
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        for (k, (yaw, tilt, mirrored)) in [
            (0.0, Vec2::ZERO, false),
            (0.3, Vec2::new(0.05, -0.02), true),
            (1.9, Vec2::new(-0.1, 0.04), false),
            (std::f32::consts::FRAC_PI_2, Vec2::ZERO, true),
        ]
        .into_iter()
        .enumerate()
        {
            let (mut stores, _) = stores();
            let entry = entry(Vec2::new(-3.1, 7.7 + k as f32), yaw, 0.002, tilt, mirrored);
            stores.stamp(&entry, &shoes, &LAW);
            let (low, high) = entry.world_bounds(&shoes[0]);
            let span = Span::covering(low - 0.2, high + 0.2, NEAR_TEXEL_M);
            let mut drawn: HashMap<IVec2, [u8; 2]> = HashMap::default();
            stores.log.draw(span, |g, value| {
                drawn.insert(g, value);
            });
            let mut printed = 0;
            for j in span.min.y..span.max.y {
                for i in span.min.x..span.max.x {
                    let g = IVec2::new(i, j);
                    let world = (g.as_vec2() + 0.5) * NEAR_TEXEL_M;
                    let want = entry.texel_at(&shoes[0], world, &LAW).unwrap_or([0, 0]);
                    let got = drawn.get(&g).copied().unwrap_or([0, 0]);
                    // The walk adds where the point law multiplies; a rounding apart at most.
                    assert!(
                        want[0].abs_diff(got[0]) <= 1 && (want[1] == 0) == (got[1] == 0),
                        "case {k}, texel {g}: law {want:?}, walk {got:?}"
                    );
                    printed += usize::from(got != [0, 0]);
                }
            }
            assert!(printed > 30, "case {k} prints a sole: {printed} texels");
        }
    }

    /// A deterministic scatter of stamps over `±spread` m around `centre`, fixture data.
    fn scatter(centre: Vec2, spread: f32, count: u32, seed: u32) -> Vec<StampEntry> {
        let mut state = seed.wrapping_mul(2_654_435_761).max(1);
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as f32 / u32::MAX as f32
        };
        (0..count)
            .map(|_| {
                let datum = centre + (Vec2::new(next(), next()) * 2.0 - 1.0) * spread;
                let yaw = next() * std::f32::consts::TAU;
                let tilt = (Vec2::new(next(), next()) - 0.5) * 0.2;
                StampEntry::new(
                    Placement {
                        datum,
                        yaw,
                        lift: 0.01 * next(),
                        tilt,
                        shoe: 0,
                        mirrored: next() < 0.5,
                    },
                    0.25 + 0.75 * next(),
                    40.0 + 200.0 * next(),
                )
            })
            .collect()
    }

    /// Every texel of the window, as the GPU holds it, against the history.
    fn assert_gpu_is_history(stores: &PrintStores, gpu: &Gpu) -> usize {
        let window = stores.near.window().expect("placed");
        let mut history: HashMap<IVec2, [u8; 2]> = HashMap::default();
        stores.log.draw(window, |g, value| {
            history.insert(g, value);
        });
        let mut printed = 0;
        for j in window.min.y..window.max.y {
            for i in window.min.x..window.max.x {
                let g = IVec2::new(i, j);
                let want = history.get(&g).copied().unwrap_or([0, 0]);
                assert_eq!(gpu.near_at(g), want, "texel {g}");
                printed += usize::from(want != [0, 0]);
            }
        }
        printed
    }

    /// THE RECENTRE'S CONTRACT: the near store's GPU copy is always exactly the history over its
    /// window. Stamps land while the window sits at A, the eye leaves for B (most of the way across
    /// the window, so a strip of A survives and the rest is replaced) where more stamps land, and
    /// comes back. Every texel of the GPU copy must then equal the history, and equal what a fresh
    /// store placed at A uploads: a recentre that forgot to clear the entering cells, cleared the
    /// wrong slot of the torus, or skipped a cell whose old marks needed zeroing breaks one or the
    /// other. The stamps straddle both windows and the cell edges by construction.
    #[test]
    fn a_recentre_away_and_back_holds_the_history_over_the_window() {
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        let (mut stores, mut gpu) = stores();
        let (a, b) = (Vec2::new(10.0, -20.0), Vec2::new(70.0, 15.0));
        assert!(stores.recentre(a));
        for entry in scatter((a + b) * 0.5, 60.0, 3000, 1) {
            stores.stamp(&entry, &shoes, &LAW);
        }
        flush(&mut stores, &mut gpu);
        assert!(stores.recentre(b), "the eye left the slack");
        flush(&mut stores, &mut gpu);
        assert_gpu_is_history(&stores, &gpu);
        for entry in scatter((a + b) * 0.5, 60.0, 3000, 2) {
            stores.stamp(&entry, &shoes, &LAW);
        }
        flush(&mut stores, &mut gpu);
        assert!(stores.recentre(a), "and came back");
        flush(&mut stores, &mut gpu);
        let printed = assert_gpu_is_history(&stores, &gpu);
        assert!(
            printed > 10_000,
            "the window holds prints to compare: {printed}"
        );

        let mut fresh = Gpu::new(WORLD_SIDE);
        let mut placed = NearStore::default();
        placed.recentre(a);
        fresh.apply(&placed.take_uploads(&stores.log), WORLD_SIDE);
        assert_eq!(
            fresh.near, gpu.near,
            "a fresh store uploads the same window"
        );
    }

    /// Recentre relies on exact history even after repeated pivots. Checkpointing must bound
    /// hot raw cells without losing older texels, including across negative cell boundaries;
    /// an immortal entry vector or discarding cold checkpoints fails here.
    #[test]
    fn compacted_history_is_spatially_bounded_and_replays_every_texel() {
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        let (mut stores, mut gpu) = stores();
        stores.recentre(Vec2::ZERO);
        for entry in
            scatter(Vec2::splat(-6.0), 1.0, 4096, 3)
                .into_iter()
                .chain(std::iter::repeat_n(
                    entry(Vec2::splat(-6.0), 0.4, 0.0, Vec2::ZERO, false),
                    8192,
                ))
        {
            stores.stamp(&entry, &shoes, &LAW);
        }
        flush(&mut stores, &mut gpu);
        let direct = gpu.near.clone();
        for k in 0..ACTIVE_CELLS + 1 {
            let at = Vec2::new(
                200.0 + (k % 20) as f32 * 12.0,
                200.0 + (k / 20) as f32 * 12.0,
            );
            stores.stamp(&entry(at, 0.0, 0.0, Vec2::ZERO, false), &shoes, &LAW);
        }
        assert!(
            stores
                .log
                .cells
                .values()
                .any(|cell| matches!(cell.checkpoint, Checkpoint::Runs(_)))
        );
        assert!(stores.log.active.len() <= ACTIVE_CELLS);
        stores.recentre(Vec2::splat(200.0));
        flush(&mut stores, &mut gpu);
        stores.recentre(Vec2::ZERO);
        flush(&mut stores, &mut gpu);
        assert_eq!(
            direct, gpu.near,
            "the torus restores compacted history after a full eviction"
        );
        let new_entry = entry(Vec2::new(-5.4, -6.1), 0.7, 0.01, Vec2::ZERO, false);
        stores.stamp(&new_entry, &shoes, &LAW);
        flush(&mut stores, &mut gpu);
        assert_gpu_is_history(&stores, &gpu);
        let noisy: Vec<_> = (0..CELL * CELL).map(|i| [(i % 2) as u8, 255]).collect();
        assert!(
            matches!(Checkpoint::packed(noisy), Checkpoint::Raw(_)),
            "incompressible checkpoints cannot grow beyond raw cell storage"
        );
    }

    /// Uploads are the GPU's actual changes. An empty startup or recentre sends nothing (wgpu
    /// zero-initialises the image), while an old print the window wraps away from must be cleared,
    /// or it becomes a ghost mark at the slot's new place; and a slot cleared once is not cleared
    /// again.
    #[test]
    fn recentre_uploads_only_changed_tiles_and_clears_old_prints() {
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        let (mut stores, mut gpu) = stores();
        stores.recentre(Vec2::ZERO);
        assert!(
            flush(&mut stores, &mut gpu).is_empty(),
            "wgpu initializes the print image to zero"
        );
        stores.stamp(
            &entry(Vec2::ZERO, 0.0, 0.0, Vec2::ZERO, false),
            &shoes,
            &LAW,
        );
        assert!(!flush(&mut stores, &mut gpu).is_empty());
        stores.recentre(Vec2::splat(200.0));
        let clear: Vec<_> = flush(&mut stores, &mut gpu)
            .into_iter()
            .filter(|u| u.store == Store::Near)
            .collect();
        assert!(
            !clear.is_empty(),
            "old nonzero storage slots must be cleared on the GPU"
        );
        assert!(clear.iter().all(|u| u.data.iter().all(|&b| b == 0)));
        stores.recentre(Vec2::ZERO);
        let back = flush(&mut stores, &mut gpu);
        assert!(
            back.iter().all(|u| u.data.iter().any(|&b| b != 0)),
            "coming back re-sends the print, and no slot is cleared twice"
        );
        assert_gpu_is_history(&stores, &gpu);
    }

    /// The press and the pressure are MAX-combined, so a stamp laid twice changes nothing, the
    /// store holds the deepest of two overlapping prints whatever order they arrive in, and a
    /// stamp further into its press only ever deepens a print.
    #[test]
    fn overlapping_stamps_keep_the_deepest_in_any_order() {
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        let shallow = entry(Vec2::new(1.0, 1.0), 0.3, 0.02, Vec2::ZERO, false);
        let deep = entry(Vec2::new(1.05, 1.0), 0.3, 0.0, Vec2::ZERO, false);
        let draw = |order: &[StampEntry]| {
            let (mut stores, mut gpu) = stores();
            stores.recentre(Vec2::ZERO);
            for entry in order {
                stores.stamp(entry, &shoes, &LAW);
            }
            flush(&mut stores, &mut gpu);
            gpu
        };
        let (one, other) = (draw(&[shallow, deep, deep]), draw(&[deep, shallow]));
        let g = (Vec2::new(1.05, 1.0) / NEAR_TEXEL_M).floor().as_ivec2();
        assert_eq!(one.near_at(g), other.near_at(g));
        assert_eq!(
            one.near_at(g),
            [255, KPA as u8],
            "the deep sole rests on the ground there, fully pressed"
        );
        let half = StampEntry::new(
            Placement {
                datum: Vec2::new(1.05, 1.0),
                yaw: 0.3,
                lift: 0.0,
                tilt: Vec2::ZERO,
                shoe: 0,
                mirrored: false,
            },
            0.5,
            KPA,
        );
        assert_eq!(draw(&[half]).near_at(g)[0], 128, "half pressed");
        assert_eq!(draw(&[deep, half]).near_at(g)[0], 255, "never shallower");
    }

    /// The world layer counts a near footprint texel once, at every sample phase: a sole visible
    /// in the near store never vanishes from the world layer, and overprinting a track is union,
    /// not accumulated darkness.
    #[test]
    fn the_world_layer_counts_each_footprint_texel_once() {
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        let (mut stores, mut gpu) = stores();
        let texel_m = 1500.0 / WORLD_SIDE as f32;
        for phase in 0..8 {
            let datum = Vec2::new(phase as f32 * 2.0, phase as f32 * texel_m / 8.0);
            stores.stamp(&entry(datum, 0.0, 0.0, Vec2::ZERO, false), &shoes, &LAW);
            let cell = ((datum + 750.0) / texel_m).floor().as_uvec2();
            let covered: u32 = (cell.y - 1..=cell.y + 1)
                .flat_map(|j| (cell.x - 2..=cell.x + 2).map(move |i| (i, j)))
                .map(|(i, j)| u32::from(stores.world.coverage(0, i, j)))
                .sum();
            assert!(
                covered > 0,
                "phase {phase}: a grounded shoe cannot disappear in the world layer"
            );
        }
        assert!(!flush(&mut stores, &mut gpu).is_empty());
        let before: Vec<_> = stores.world.counts.clone();
        stores.stamp(
            &entry(Vec2::ZERO, 0.0, 0.0, Vec2::ZERO, false),
            &shoes,
            &LAW,
        );
        assert_eq!(
            stores.world.counts, before,
            "overprinting is union, not accumulated darkness"
        );
        assert!(stores.take_uploads().is_empty());
        for w in 0..WORLD_SIDE {
            assert!(
                (7..=8).contains(&stores.world.per_axis[w as usize]),
                "world texel {w} holds 7 or 8 near texels along an axis"
            );
        }
    }

    /// The world layer's GPU copy is rebuilt up the WHOLE mip chain over the tiles a track touched,
    /// each level the rounded mean of the one below, and nothing is sent twice. A level the rebuild
    /// skipped would keep showing the ground untrodden at the range that level is sampled at.
    #[test]
    fn the_world_layer_mips_carry_a_track_up_the_chain() {
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        let mut stores = PrintStores::new(64.0, 256);
        for k in 0..40 {
            let along = Vec2::new(-20.0, -28.0 + 0.13 * k as f32);
            stores.stamp(&entry(along, 0.0, 0.0, Vec2::ZERO, false), &shoes, &LAW);
        }
        let uploads = stores.take_uploads();
        assert!(
            uploads
                .iter()
                .any(|u| u.store == Store::World && u.level == 0),
            "the new footprint is uploaded"
        );
        let world: Vec<_> = uploads.iter().filter(|u| u.store == Store::World).collect();
        let unique: std::collections::BTreeSet<_> = world
            .iter()
            .map(|u| (u.level, u.origin.x, u.origin.y))
            .collect();
        assert_eq!(
            unique.len(),
            world.len(),
            "each dirty tile is uploaded once per mip"
        );
        let levels: std::collections::BTreeSet<u32> = world.iter().map(|u| u.level).collect();
        assert_eq!(
            levels,
            (0..stores.world.levels()).collect(),
            "every level is sent"
        );
        let (i, j) = (48 / 2, 26 / 2);
        assert!(
            stores.world.coverage(1, i, j) > 0,
            "the track shows at level 1"
        );
        for level in 1..stores.world.levels() {
            let (i, j) = (i >> (level - 1), j >> (level - 1));
            let below: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)]
                .into_iter()
                .map(|(di, dj)| u32::from(stores.world.coverage(level - 1, 2 * i + di, 2 * j + dj)))
                .sum();
            assert_eq!(
                u32::from(stores.world.coverage(level, i, j)),
                (below + 2) / 4,
                "level {level} is the mean of level {}",
                level - 1,
            );
        }
        assert!(
            stores.take_uploads().is_empty(),
            "nothing new, nothing sent"
        );
    }

    /// Manual headless CPU measurement of contact, resampling, the press, history, the raster,
    /// world counts, mip rebuilds and upload packing at 30 synthetic tanks under pivot. The
    /// renderer and ECS query overhead are excluded; no GPU timing or frame-budget assertion is
    /// implied.
    #[test]
    #[ignore = "manual headless CPU measurement; run with --ignored --nocapture"]
    fn thirty_tank_pivot_cpu_and_upload_measurement() {
        use crate::terrain_grid::{HeightGrid, TerrainExtent};
        use crate::track_prints::stamper::{ShoeKey, ShoeMemory, contact};
        use bevy::math::{Affine3A, Quat};
        use bevy::prelude::Entity;
        use std::sync::Arc;
        use std::time::Instant;
        let frames: usize = std::env::var("OVERMATCH_PRINT_BENCH_FRAMES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(180);
        assert!(frames > 0);
        let grid = HeightGrid::new(
            vec![0.0; 1025 * 1025].into(),
            1025,
            TerrainExtent {
                world_size_m: 1500.0,
                height_offset_m: 0.0,
                height_span_m: 1.0,
            },
        );
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        // Synthetic fixture geometry: the link pitch the press ramps over.
        let pitch = 0.13;
        for (layout, spacing) in [("clustered", 12.0), ("dispersed", 200.0)] {
            let mut memory: Vec<_> = (0..30).map(|_| ShoeMemory::default()).collect();
            let mut stores = PrintStores::new(1500.0, WORLD_SIDE);
            stores.recentre(Vec2::ZERO);
            let cold_bytes: usize = stores.take_uploads().iter().map(|u| u.data.len()).sum();
            let mut times = Vec::new();
            let mut bytes = Vec::new();
            // Per phase, summed over the run: contacts and the press, stamping, uploads.
            let mut phases = [0.0_f64; 3];
            let mut laid = Vec::new();
            for frame in 0..frames {
                let start = Instant::now();
                let yaw = frame as f32 * 0.018; // ASSUMED 1.08 rad/s at 60 Hz.
                for (tank, memory) in memory.iter_mut().enumerate() {
                    let centre = (Vec2::new((tank % 6) as f32, (tank / 6) as f32)
                        - Vec2::new(2.5, 2.0))
                        * spacing;
                    for side in 0..2 {
                        for k in 0..27 {
                            let local = Vec2::new(
                                if side == 0 { -1.4 } else { 1.4 },
                                (k as f32 - 13.0) * 0.13,
                            ); // Synthetic fixture geometry.
                            let at = centre + Vec2::from_angle(-yaw).rotate(local);
                            let pose = Affine3A::from_rotation_translation(
                                Quat::from_rotation_y(yaw),
                                Vec3::new(at.x, 0.0, at.y),
                            );
                            let c = contact(
                                pose,
                                shoes[0].bounds(false),
                                shoes[0].datum_y(),
                                LAW.contact_m,
                                |p| Some(grid.height_at(p.x, p.y)),
                            )
                            .unwrap();
                            let id = ShoeKey::new(Entity::PLACEHOLDER, side * 27 + k);
                            let hull_local = Vec3::new(local.x, 0.0, local.y);
                            if let Some(sweep) = memory.observe(id, &c, hull_local, pitch) {
                                laid.extend(sweep.map(|(c, press)| {
                                    StampEntry::new(c.placement(0, false), press, KPA)
                                }));
                            }
                        }
                    }
                }
                let contacts = Instant::now();
                for entry in laid.drain(..) {
                    stores.stamp(&entry, &shoes, &LAW);
                }
                let stamped = Instant::now();
                let uploads = stores.take_uploads();
                bytes.push(uploads.iter().map(|u| u.data.len()).sum::<usize>());
                // flush converts Vec payloads into shared extraction buffers too.
                for upload in uploads {
                    std::hint::black_box(Arc::<[u8]>::from(upload.data));
                }
                let done = Instant::now();
                for (phase, (from, to)) in
                    phases
                        .iter_mut()
                        .zip([(start, contacts), (contacts, stamped), (stamped, done)])
                {
                    *phase += (to - from).as_secs_f64() * 1000.0;
                }
                times.push((done - start).as_secs_f64() * 1000.0);
            }
            times.sort_by(f64::total_cmp);
            let history_bytes: usize = stores
                .log
                .cells
                .values()
                .map(|c| match &c.checkpoint {
                    Checkpoint::Empty => 0,
                    Checkpoint::Runs(r) => std::mem::size_of_val(&**r),
                    Checkpoint::Raw(r) => std::mem::size_of_val(&**r),
                })
                .sum();
            eprintln!(
                "MEASURED {layout}, {frames} frames, 30 x 54 grounded shoes: mean {:.3} ms, p95 {:.3} ms, max {:.3} ms; uploads mean {} B, max {} B, cold {} B; history payload {} B in {} cells, logical stamps {}",
                times.iter().sum::<f64>() / frames as f64,
                times[(frames - 1) * 95 / 100],
                times[frames - 1],
                bytes.iter().sum::<usize>() / frames,
                bytes.iter().max().unwrap(),
                cold_bytes,
                history_bytes,
                stores.log.cells.len(),
                stores.log.len()
            );
            let [contacts, stamping, uploads] = phases.map(|total| total / frames as f64);
            eprintln!(
                "MEASURED {layout} phase means: contacts and press {contacts:.3} ms, stamping \
                 {stamping:.3} ms, uploads {uploads:.3} ms"
            );
            assert!(stores.log.active.len() <= ACTIVE_CELLS);
            assert!(
                stores.take_uploads().is_empty(),
                "upload packing consumes dirtiness exactly once"
            );
        }
    }
}
