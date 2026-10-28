//! The prints' stores, all CPU-side: the stamp log every store is redrawn from, the near store
//! around the camera, and the world layer over the whole map. Each GPU texture is a copy of a store
//! here, written tile by tile ([`Upload`]); nothing is ever read back.
//!
//! # A stamp, at one point of ground
//!
//! A [`StampEntry`] places a shoe's pooled underside ([`ClearanceStamp`]) on the ground: its datum
//! point and heading in world XZ, and the plane its underside makes with the ground under it, as a
//! lift at the canonical origin and a tilt across and along the shoe. At a world point the shoe's
//! underside clears the ground by the underside's own height there plus the plane's: that
//! CLEARANCE is what the stores keep, and a pack whose print depth is `D` sinks the ground by
//! `max(0, D − clearance)` (`terrain_blend.wgsl`). The plane is normalized at the stamper so a
//! grounded shoe touches at its lowest corner (`super::stamper`).
//!
//! # The near store
//!
//! [`NEAR_SIDE`]² texels of [`NEAR_TEXEL_M`], two bytes each: R the PRESS, `1 − clearance /
//! range` ([`PrintLaw::range_m`]), so zero means no print and the largest press is the deepest
//! print; G the FOOTPRINT, full wherever a grounded shoe's sole lay. Both combine by MAX, which is
//! order-free, so any set of stamps draws the same texels in any order and in any pieces.
//!
//! It is TOROIDAL: global texel `g` lives at storage `g mod side`, and the window it currently
//! holds, [`NearStore::window`], follows the camera in steps of [`RECENTRE_STEP`]. A recentre
//! clears the texels that entered the window and redraws them from the log, so the store is always
//! exactly what a fresh draw of the log over the window gives.
//!
//! # The world layer
//!
//! [`WORLD_SIDE`]² texels over the map's square. A texel keeps which of its `2 × 2` sample points
//! a grounded sole has covered, and the GPU copy carries the covered fraction with a full mip chain
//! the CPU builds tile by tile. Union, not sum: a track laid over and over stays one track.

use std::collections::HashMap;

use bevy::math::{IVec2, UVec2, Vec2};

use super::stamp::{ClearanceStamp, OPEN_M};

/// Texels across the near store. DERIVED from the approaches note's §4 starting point.
pub(crate) const NEAR_SIDE: u32 = 2048;
/// Metres across the near store. ASSUMED (approaches note §4): about twice the camera's farthest
/// orbit plus the distance the normal survives.
pub(crate) const NEAR_SPAN_M: f32 = 96.0;
/// The near store's texel, m: DERIVED, 4.6875 cm.
pub(crate) const NEAR_TEXEL_M: f32 = NEAR_SPAN_M / NEAR_SIDE as f32;
/// Texels the window moves by, at least, when it recentres: the window's origin is always a
/// multiple of this. ASSUMED: 6 m, a strip small enough to redraw in a frame.
pub(crate) const RECENTRE_STEP: i32 = 128;
/// How far the eye may wander from the window's centre, in texels along either axis, before the
/// window recentres. Above the step, so an eye on a step boundary does not flap. ASSUMED: 9 m,
/// which leaves 39 m of the window ahead of the eye in every direction.
pub(crate) const RECENTRE_SLACK: i32 = 192;
/// Texels across the world layer. DERIVED from the approaches note's §4 starting point: 0.37 m
/// on the shipped 1500 m map.
pub(crate) const WORLD_SIDE: u32 = 4096;
/// The upload tile: a store is written to the GPU in squares of this many texels (fewer at a mip
/// level smaller than one).
const TILE: u32 = 64;
/// The log's index cell, m. ASSUMED: a few shoe lengths, small next to a recentre strip.
const LOG_CELL_M: f32 = 8.0;
/// The world layer's coverage per number of covered sample points.
const COVERED: [u8; 5] = [0, 64, 128, 191, 255];

/// How stamps turn into store texels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PrintLaw {
    /// The clearance the near store's press spans, m: a point a sole clears by this much or more
    /// prints on no pack. The deepest print any pack takes, so no pack's print is clipped.
    pub(crate) range_m: f32,
    /// How far a sole's plane may clear the ground and still lie on it, m: the footprint's edge,
    /// and the stamper's test for a grounded shoe.
    pub(crate) contact_m: f32,
}

/// One stamp in the log: 16 bytes.
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
    /// The plane's slope across (`x`) and along (`z`) the shoe, 1/256 m per m.
    tilt: [i8; 2],
    /// Bit 0: the shoe is the mirror of its stamp (the left track).
    flags: u8,
    /// Which shoe stamp: an index into the stamps the stores are drawn with.
    shoe: u8,
}

const YAW_UNITS: f32 = 65536.0 / std::f32::consts::TAU;
const LIFT_UNITS: f32 = 4096.0;
const TILT_UNITS: f32 = 256.0;
const MIRRORED: u8 = 1;

impl StampEntry {
    /// A stamp of shoe `shoe` with its datum at world `datum`, its canonical `+z` heading `yaw`
    /// (rad from `+Z` toward `+X`), and its underside plane `lift + tilt · (x, z)` over the ground.
    pub(crate) fn new(
        datum: Vec2,
        yaw: f32,
        lift: f32,
        tilt: Vec2,
        shoe: u8,
        mirrored: bool,
    ) -> Self {
        let tilt = tilt * TILT_UNITS;
        Self {
            x: datum.x,
            z: datum.y,
            yaw: (yaw.rem_euclid(std::f32::consts::TAU) * YAW_UNITS).round() as u32 as u16,
            lift: (lift * LIFT_UNITS)
                .round()
                .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16,
            tilt: [tilt.x, tilt.y].map(|t| t.round().clamp(-127.0, 127.0) as i8),
            flags: if mirrored { MIRRORED } else { 0 },
            shoe,
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

    /// The shoe's canonical `x` and `z` axes in world XZ.
    fn axes(&self) -> (Vec2, Vec2) {
        let (sin, cos) = (f32::from(self.yaw) / YAW_UNITS).sin_cos();
        (Vec2::new(cos, -sin), Vec2::new(sin, cos))
    }

    /// Canonical `(x, z)` of world XZ `world`.
    fn canonical(&self, world: Vec2) -> Vec2 {
        let (across, along) = self.axes();
        let d = world - self.datum();
        Vec2::new(d.dot(across), d.dot(along))
    }

    /// The underside plane's clearance at canonical `c`, m.
    fn plane(&self, c: Vec2) -> f32 {
        f32::from(self.lift) / LIFT_UNITS
            + (f32::from(self.tilt[0]) * c.x + f32::from(self.tilt[1]) * c.y) / TILT_UNITS
    }

    /// The world XZ box the shoe's stamp covers: low, high.
    pub(crate) fn world_bounds(&self, shoe: &ClearanceStamp) -> (Vec2, Vec2) {
        let (low, high) = shoe.bounds(self.mirrored());
        let (across, along) = self.axes();
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

    /// At world XZ `world`: the sole's clearance over the ground, m, and whether the point is
    /// footprint (under the sole, where its plane lies on the ground). `None` off the sole and
    /// over an opening.
    fn at(&self, shoe: &ClearanceStamp, world: Vec2, law: &PrintLaw) -> Option<(f32, bool)> {
        let c = self.canonical(world);
        let height = shoe.height(c.x, c.y, self.mirrored());
        if height >= OPEN_M {
            return None;
        }
        let plane = self.plane(c);
        Some((height + plane, plane <= law.contact_m))
    }
}

/// The press a clearance stores: `1 − clearance / range`, clamped, in 1/255.
fn press(clearance: f32, range_m: f32) -> u8 {
    ((1.0 - clearance / range_m).clamp(0.0, 1.0) * 255.0).round() as u8
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

    /// The texels of `self` outside `other`, as at most four disjoint spans.
    fn minus(self, other: Self) -> Vec<Self> {
        let Some(shared) = self.intersect(other) else {
            return vec![self];
        };
        let mut out = Vec::with_capacity(4);
        if self.min.x < shared.min.x {
            out.push(Self {
                min: self.min,
                max: IVec2::new(shared.min.x, self.max.y),
            });
        }
        if shared.max.x < self.max.x {
            out.push(Self {
                min: IVec2::new(shared.max.x, self.min.y),
                max: self.max,
            });
        }
        if self.min.y < shared.min.y {
            out.push(Self {
                min: IVec2::new(shared.min.x, self.min.y),
                max: IVec2::new(shared.max.x, shared.min.y),
            });
        }
        if shared.max.y < self.max.y {
            out.push(Self {
                min: IVec2::new(shared.min.x, shared.max.y),
                max: IVec2::new(shared.max.x, self.max.y),
            });
        }
        out
    }

    /// The texels of `texel_m` whose centres a world box from `low` to `high` holds.
    fn covering(low: Vec2, high: Vec2, texel_m: f32) -> Self {
        Self {
            min: (low / texel_m - 0.5).ceil().as_ivec2(),
            max: (high / texel_m - 0.5).floor().as_ivec2() + 1,
        }
    }
}

/// Every stamp laid this battle, in the order laid, indexed by the [`LOG_CELL_M`] cells their
/// world boxes touch.
#[derive(Default)]
pub(crate) struct StampLog {
    entries: Vec<StampEntry>,
    cells: HashMap<IVec2, Vec<u32>>,
}

impl StampLog {
    pub(crate) fn push(&mut self, entry: StampEntry, shoe: &ClearanceStamp) {
        let index = self.entries.len() as u32;
        self.entries.push(entry);
        let (low, high) = entry.world_bounds(shoe);
        let (low, high) = (Self::cell(low), Self::cell(high));
        for j in low.y..=high.y {
            for i in low.x..=high.x {
                self.cells.entry(IVec2::new(i, j)).or_default().push(index);
            }
        }
    }

    fn cell(world: Vec2) -> IVec2 {
        (world / LOG_CELL_M).floor().as_ivec2()
    }

    /// The stamps whose world boxes may touch the box from `low` to `high`, each once, in log
    /// order.
    pub(crate) fn near(&self, low: Vec2, high: Vec2) -> Vec<StampEntry> {
        let (low, high) = (Self::cell(low), Self::cell(high));
        let mut found: Vec<u32> = Vec::new();
        for j in low.y..=high.y {
            for i in low.x..=high.x {
                if let Some(indices) = self.cells.get(&IVec2::new(i, j)) {
                    found.extend_from_slice(indices);
                }
            }
        }
        found.sort_unstable();
        found.dedup();
        found
            .into_iter()
            .map(|index| self.entries[index as usize])
            .collect()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
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

/// The near store ([module doc](self)).
pub(crate) struct NearStore {
    /// The window's low corner in global texels; `None` before the first placement.
    origin: Option<IVec2>,
    /// `[press, footprint]` per storage texel.
    texels: Vec<[u8; 2]>,
    /// Storage tiles written since the last [`Self::take_uploads`].
    dirty: Vec<bool>,
}

impl Default for NearStore {
    fn default() -> Self {
        let tiles = (NEAR_SIDE / TILE) as usize;
        Self {
            origin: None,
            texels: vec![[0, 0]; (NEAR_SIDE * NEAR_SIDE) as usize],
            dirty: vec![false; tiles * tiles],
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

    fn slot(g: IVec2) -> (usize, UVec2) {
        let side = NEAR_SIDE as i32;
        let s = UVec2::new(g.x.rem_euclid(side) as u32, g.y.rem_euclid(side) as u32);
        ((s.y * NEAR_SIDE + s.x) as usize, s)
    }

    fn mark(&mut self, storage: UVec2) {
        let tiles = NEAR_SIDE / TILE;
        self.dirty[((storage.y / TILE) * tiles + storage.x / TILE) as usize] = true;
    }

    /// The texel at global `g`, as stored: meaningful only inside [`Self::window`].
    #[cfg(test)]
    pub(crate) fn texel(&self, g: IVec2) -> [u8; 2] {
        self.texels[Self::slot(g).0]
    }

    /// Draw one stamp into the window.
    pub(crate) fn stamp(&mut self, entry: &StampEntry, shoes: &[ClearanceStamp], law: &PrintLaw) {
        if let Some(window) = self.window() {
            self.draw(entry, shoes, law, window);
        }
    }

    fn draw(&mut self, entry: &StampEntry, shoes: &[ClearanceStamp], law: &PrintLaw, clip: Span) {
        let Some(shoe) = shoes.get(entry.shoe()) else {
            return;
        };
        let (low, high) = entry.world_bounds(shoe);
        let Some(span) = Span::covering(low, high, NEAR_TEXEL_M).intersect(clip) else {
            return;
        };
        for j in span.min.y..span.max.y {
            for i in span.min.x..span.max.x {
                let g = IVec2::new(i, j);
                let world = (g.as_vec2() + 0.5) * NEAR_TEXEL_M;
                let Some((clearance, footprint)) = entry.at(shoe, world, law) else {
                    continue;
                };
                let written = [
                    press(clearance, law.range_m),
                    if footprint { 255 } else { 0 },
                ];
                if written == [0, 0] {
                    continue;
                }
                let (index, storage) = Self::slot(g);
                let texel = &mut self.texels[index];
                let merged = [texel[0].max(written[0]), texel[1].max(written[1])];
                if merged != *texel {
                    *texel = merged;
                    self.mark(storage);
                }
            }
        }
    }

    /// Clear `span` (inside the window) and draw every logged stamp over it.
    fn redraw(&mut self, span: Span, log: &StampLog, shoes: &[ClearanceStamp], law: &PrintLaw) {
        for j in span.min.y..span.max.y {
            for i in span.min.x..span.max.x {
                let (index, storage) = Self::slot(IVec2::new(i, j));
                self.texels[index] = [0, 0];
                self.mark(storage);
            }
        }
        let low = span.min.as_vec2() * NEAR_TEXEL_M;
        let high = span.max.as_vec2() * NEAR_TEXEL_M;
        for entry in log.near(low, high) {
            self.draw(&entry, shoes, law, span);
        }
    }

    /// Follow the eye at world XZ `eye`: once it has wandered more than [`RECENTRE_SLACK`] from
    /// the window's centre (or on the first call), move the window onto it and redraw the texels
    /// that entered. Whether the window moved.
    pub(crate) fn recentre(
        &mut self,
        eye: Vec2,
        log: &StampLog,
        shoes: &[ClearanceStamp],
        law: &PrintLaw,
    ) -> bool {
        let half = NEAR_SIDE as i32 / 2;
        let eye = (eye / NEAR_TEXEL_M).floor().as_ivec2();
        if let Some(origin) = self.origin
            && (eye - (origin + half)).abs().max_element() <= RECENTRE_SLACK
        {
            return false;
        }
        let origin = (eye - half).div_euclid(IVec2::splat(RECENTRE_STEP)) * RECENTRE_STEP;
        let window = Span {
            min: origin,
            max: origin + NEAR_SIDE as i32,
        };
        let entered = match self.window() {
            Some(old) => window.minus(old),
            None => vec![window],
        };
        self.origin = Some(origin);
        for span in entered {
            self.redraw(span, log, shoes, law);
        }
        true
    }

    /// The tiles written since the last call, as uploads of the GPU copy's single level.
    pub(crate) fn take_uploads(&mut self) -> Vec<Upload> {
        let tiles = NEAR_SIDE / TILE;
        let mut out = Vec::new();
        for (index, dirty) in self.dirty.iter_mut().enumerate() {
            if !std::mem::take(dirty) {
                continue;
            }
            let tile = UVec2::new(index as u32 % tiles, index as u32 / tiles) * TILE;
            let mut data = Vec::with_capacity((TILE * TILE * 2) as usize);
            for j in tile.y..tile.y + TILE {
                let row = (j * NEAR_SIDE + tile.x) as usize;
                for texel in &self.texels[row..row + TILE as usize] {
                    data.extend_from_slice(texel);
                }
            }
            out.push(Upload {
                store: Store::Near,
                level: 0,
                origin: tile,
                size: UVec2::splat(TILE),
                data,
            });
        }
        out
    }
}

/// The world layer ([module doc](self)).
pub(crate) struct WorldStore {
    /// World XZ of texel `(0, 0)`'s low corner.
    origin_m: Vec2,
    texel_m: f32,
    side: u32,
    /// Level 0: the `2 × 2` sample points of each texel a sole has covered, one bit each.
    masks: Vec<u8>,
    /// Levels 1 and up, coverage in 1/255: `mips[k − 1]` is level `k`.
    mips: Vec<Vec<u8>>,
    /// Level-0 tiles written since the last [`Self::take_uploads`].
    dirty: Vec<bool>,
}

impl WorldStore {
    /// A world layer of `side`² texels over the square of `size_m` centred on the origin.
    pub(crate) fn new(size_m: f32, side: u32) -> Self {
        let tiles = side.div_ceil(TILE) as usize;
        let mips = (1..Self::levels_of(side))
            .map(|k| vec![0; ((side >> k) * (side >> k)) as usize])
            .collect();
        Self {
            origin_m: Vec2::splat(-0.5 * size_m),
            texel_m: size_m / side as f32,
            side,
            masks: vec![0; (side * side) as usize],
            mips,
            dirty: vec![false; tiles * tiles],
        }
    }

    fn levels_of(side: u32) -> u32 {
        side.ilog2() + 1
    }

    /// The GPU copy's mip levels.
    pub(crate) fn levels(&self) -> u32 {
        Self::levels_of(self.side)
    }

    /// Coverage of texel `(i, j)` at mip `level`, in 1/255.
    pub(crate) fn coverage(&self, level: u32, i: u32, j: u32) -> u8 {
        if level == 0 {
            COVERED[self.masks[(j * self.side + i) as usize].count_ones() as usize]
        } else {
            self.mips[level as usize - 1][(j * (self.side >> level) + i) as usize]
        }
    }

    /// Draw one stamp's footprint.
    pub(crate) fn stamp(&mut self, entry: &StampEntry, shoes: &[ClearanceStamp], law: &PrintLaw) {
        let Some(shoe) = shoes.get(entry.shoe()) else {
            return;
        };
        let (low, high) = entry.world_bounds(shoe);
        let to_texel = |w: Vec2| ((w - self.origin_m) / self.texel_m).floor().as_ivec2();
        let last = self.side as i32 - 1;
        let (low, high) = (
            to_texel(low).clamp(IVec2::ZERO, IVec2::splat(last)),
            to_texel(high).clamp(IVec2::ZERO, IVec2::splat(last)),
        );
        let tiles = self.side.div_ceil(TILE);
        for j in low.y..=high.y {
            for i in low.x..=high.x {
                let mut bits = 0_u8;
                for (bit, offset) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)]
                    .into_iter()
                    .enumerate()
                {
                    let world = self.origin_m
                        + (Vec2::new(i as f32, j as f32) + Vec2::from(offset)) * self.texel_m;
                    if entry
                        .at(shoe, world, law)
                        .is_some_and(|(_, footprint)| footprint)
                    {
                        bits |= 1 << bit;
                    }
                }
                let mask = &mut self.masks[(j as u32 * self.side + i as u32) as usize];
                if *mask | bits != *mask {
                    *mask |= bits;
                    self.dirty[((j as u32 / TILE) * tiles + i as u32 / TILE) as usize] = true;
                }
            }
        }
    }

    /// The level-0 tiles written since the last call and every mip tile above them, rebuilt and as
    /// uploads, finest level first.
    pub(crate) fn take_uploads(&mut self) -> Vec<Upload> {
        let tiles = self.side.div_ceil(TILE);
        let mut dirty: Vec<UVec2> = self
            .dirty
            .iter_mut()
            .enumerate()
            .filter_map(|(index, dirty)| {
                std::mem::take(dirty)
                    .then(|| UVec2::new(index as u32 % tiles, index as u32 / tiles))
            })
            .collect();
        let mut out = Vec::new();
        for level in 0..self.levels() {
            let side = self.side >> level;
            let tile = TILE.min(side);
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
        range_m: 0.15,
        contact_m: 0.03,
    };

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
                StampEntry::new(datum, yaw, 0.01 * next(), tilt, 0, next() < 0.5)
            })
            .collect()
    }

    /// THE RECENTRE'S CONTRACT: the near store is always what a fresh draw of the log over its
    /// window gives. Stamps land while the window sits at A, the eye leaves for B (most of the way
    /// across the window, so a strip of A survives and the rest is redrawn) where more stamps land,
    /// and comes back. Every texel of the window must then equal a store placed at A for the
    /// first time over the same log, which is what a recentre that forgot to clear the entering
    /// strip, cleared the wrong slot of the torus, or skipped a stamp straddling a strip's edge
    /// would break. The stamps straddle both windows and the strip edges by construction.
    #[test]
    fn a_recentre_away_and_back_redraws_what_a_fresh_store_holds() {
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        let mut log = StampLog::default();
        let mut store = NearStore::default();
        let (a, b) = (Vec2::new(10.0, -20.0), Vec2::new(70.0, 15.0));
        assert!(store.recentre(a, &log, &shoes, &LAW));
        let lay = |entries: Vec<StampEntry>, log: &mut StampLog, store: &mut NearStore| {
            for entry in entries {
                log.push(entry, &shoes[0]);
                store.stamp(&entry, &shoes, &LAW);
            }
        };
        lay(scatter((a + b) * 0.5, 60.0, 3000, 1), &mut log, &mut store);
        assert!(
            store.recentre(b, &log, &shoes, &LAW),
            "the eye left the slack"
        );
        lay(scatter((a + b) * 0.5, 60.0, 3000, 2), &mut log, &mut store);
        assert!(store.recentre(a, &log, &shoes, &LAW), "and came back");

        let mut fresh = NearStore::default();
        fresh.recentre(a, &log, &shoes, &LAW);
        let window = store.window().expect("placed");
        assert_eq!(Some(window), fresh.window());
        let mut printed = 0;
        for j in window.min.y..window.max.y {
            for i in window.min.x..window.max.x {
                let g = IVec2::new(i, j);
                assert_eq!(store.texel(g), fresh.texel(g), "texel {g}");
                printed += usize::from(store.texel(g) != [0, 0]);
            }
        }
        assert!(
            printed > 10_000,
            "the window holds prints to compare: {printed}"
        );
    }

    /// The press and the footprint are MAX-combined, so a stamp laid twice changes nothing, and the
    /// near store holds the deepest of two overlapping prints whatever order they arrive in.
    #[test]
    fn overlapping_stamps_keep_the_deepest_in_any_order() {
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        let shallow = StampEntry::new(Vec2::new(1.0, 1.0), 0.3, 0.05, Vec2::ZERO, 0, false);
        let deep = StampEntry::new(Vec2::new(1.05, 1.0), 0.3, 0.0, Vec2::ZERO, 0, false);
        let draw = |order: &[StampEntry]| {
            let mut store = NearStore::default();
            store.recentre(Vec2::ZERO, &StampLog::default(), &shoes, &LAW);
            for entry in order {
                store.stamp(entry, &shoes, &LAW);
            }
            store
        };
        let (one, other) = (draw(&[shallow, deep, deep]), draw(&[deep, shallow]));
        let g = (Vec2::new(1.05, 1.0) / NEAR_TEXEL_M).floor().as_ivec2();
        assert_eq!(one.texel(g), other.texel(g));
        assert_eq!(
            one.texel(g),
            [255, 255],
            "the deep sole rests on the ground there"
        );
    }

    /// The world layer's GPU copy is rebuilt up the WHOLE mip chain over the tiles a track touched,
    /// each level the rounded mean of the one below, and nothing is sent twice. A level the rebuild
    /// skipped would keep showing the ground untrodden at the range that level is sampled at.
    #[test]
    fn the_world_layer_mips_carry_a_track_up_the_chain() {
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        let mut world = WorldStore::new(64.0, 256);
        for k in 0..40 {
            let along = Vec2::new(-20.0, -28.0 + 0.13 * k as f32);
            let entry = StampEntry::new(along, 0.0, 0.0, Vec2::ZERO, 0, false);
            world.stamp(&entry, &shoes, &LAW);
        }
        let uploads = world.take_uploads();
        assert_eq!(
            uploads.iter().filter(|u| u.level == 0).count(),
            1,
            "one level-0 tile holds the whole track",
        );
        let levels: std::collections::BTreeSet<u32> = uploads.iter().map(|u| u.level).collect();
        assert_eq!(levels, (0..world.levels()).collect(), "every level is sent");
        let (i, j) = (48 / 2, 26 / 2);
        assert!(world.coverage(1, i, j) > 0, "the track shows at level 1");
        for level in 1..world.levels() {
            let (i, j) = (i >> (level - 1), j >> (level - 1));
            let below: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)]
                .into_iter()
                .map(|(di, dj)| u32::from(world.coverage(level - 1, 2 * i + di, 2 * j + dj)))
                .sum();
            assert_eq!(
                u32::from(world.coverage(level, i, j)),
                (below + 2) / 4,
                "level {level} is the mean of level {}",
                level - 1,
            );
        }
        assert!(world.take_uploads().is_empty(), "nothing new, nothing sent");
    }
}
