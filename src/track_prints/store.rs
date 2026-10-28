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
//! [`WORLD_SIDE`]² texels over the map's square. A texel keeps which of its `8 × 8` sample points
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
/// Near texels per log cell axis. ASSUMED: 6 m cells, aligned to the global raster.
const LOG_CELL_SIDE: i32 = 128;
/// Pending stamps per cell before exact raster compaction. ASSUMED: at most 8 KiB.
const LOG_BATCH: usize = 512;
/// Samples per world-texel axis. DERIVED: on a 1500 m map their 4.58 cm spacing is
/// below the near texel's 4.69 cm, so near-scale footprints survive downsampling.
const WORLD_SAMPLES: u32 = 8;

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
    /// The plane's slope across (`x`) and along (`z`) the shoe, 1/64 m per m.
    tilt: [i8; 2],
    /// Bit 0: the shoe is the mirror of its stamp (the left track).
    flags: u8,
    /// Which shoe stamp: an index into the stamps the stores are drawn with.
    shoe: u8,
}

const YAW_UNITS: f32 = 65536.0 / std::f32::consts::TAU;
const LIFT_UNITS: f32 = 4096.0;
// DERIVED: the range must include sin(60°), the largest shoe tilt accepted by the
// stamper on flat ground, plus the ground's own slope; 1/64 spans almost ±2 m/m.
const TILT_UNITS: f32 = 64.0;
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
    fn canonical(&self, world: Vec2, (across, along): (Vec2, Vec2)) -> Vec2 {
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
    fn at(
        &self,
        shoe: &ClearanceStamp,
        world: Vec2,
        law: &PrintLaw,
        axes: (Vec2, Vec2),
    ) -> Option<(f32, bool)> {
        let c = self.canonical(world, axes);
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

/// Spatial stamp history. Each cell retains a bounded batch of operations and an exact
/// near-resolution checkpoint of earlier operations. MAX composition is associative, so
/// compaction loses neither detail nor persistence. Shoe bakes and the print law are immutable
/// while this history exists. Memory grows with visited area, never with repeat visits.
#[derive(Default)]
pub(crate) struct StampLog {
    count: usize,
    cells: HashMap<IVec2, LogCell>,
}

#[derive(Default)]
struct LogCell {
    entries: Vec<StampEntry>,
    checkpoint: Checkpoint,
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

impl LogCell {
    fn compact(&mut self, span: Span, shoes: &[ClearanceStamp], law: &PrintLaw) {
        let mut texels = vec![[0, 0]; (LOG_CELL_SIDE * LOG_CELL_SIDE) as usize];
        self.checkpoint.visit(|i, value| texels[i] = value);
        for entry in &self.entries {
            let Some(shoe) = shoes.get(entry.shoe()) else {
                continue;
            };
            raster(entry, shoe, law, span, |g, value| {
                let c = g - span.min;
                let texel = &mut texels[(c.y * LOG_CELL_SIDE + c.x) as usize];
                *texel = [texel[0].max(value[0]), texel[1].max(value[1])];
            });
        }
        self.entries.clear();
        self.checkpoint = Checkpoint::packed(texels);
    }
}

impl StampLog {
    pub(crate) fn push(&mut self, entry: StampEntry, shoes: &[ClearanceStamp], law: &PrintLaw) {
        let Some(shoe) = shoes.get(entry.shoe()) else {
            return;
        };
        self.count += 1;
        let (low, high) = entry.world_bounds(shoe);
        let span = Span::covering(low, high, NEAR_TEXEL_M);
        if span.min.x >= span.max.x || span.min.y >= span.max.y {
            return;
        }
        let (low, high) = (
            span.min.div_euclid(IVec2::splat(LOG_CELL_SIDE)),
            (span.max - 1).div_euclid(IVec2::splat(LOG_CELL_SIDE)),
        );
        for j in low.y..=high.y {
            for i in low.x..=high.x {
                let key = IVec2::new(i, j);
                let cell = self.cells.entry(key).or_default();
                cell.entries.push(entry);
                if cell.entries.len() == LOG_BATCH {
                    let min = key * LOG_CELL_SIDE;
                    cell.compact(
                        Span {
                            min,
                            max: min + LOG_CELL_SIDE,
                        },
                        shoes,
                        law,
                    );
                }
            }
        }
    }

    /// Reproduce the exact near raster in `span`, from checkpoints plus pending operations.
    fn draw(
        &self,
        span: Span,
        shoes: &[ClearanceStamp],
        law: &PrintLaw,
        mut put: impl FnMut(IVec2, [u8; 2]),
    ) {
        let low = span.min.div_euclid(IVec2::splat(LOG_CELL_SIDE));
        let high = (span.max - 1).div_euclid(IVec2::splat(LOG_CELL_SIDE));
        for j in low.y..=high.y {
            for i in low.x..=high.x {
                let key = IVec2::new(i, j);
                let Some(cell) = self.cells.get(&key) else {
                    continue;
                };
                let min = key * LOG_CELL_SIDE;
                let Some(clip) = span.intersect(Span {
                    min,
                    max: min + LOG_CELL_SIDE,
                }) else {
                    continue;
                };
                cell.checkpoint.visit(|index, value| {
                    let g = min
                        + IVec2::new(index as i32 % LOG_CELL_SIDE, index as i32 / LOG_CELL_SIDE);
                    if g.x >= clip.min.x
                        && g.x < clip.max.x
                        && g.y >= clip.min.y
                        && g.y < clip.max.y
                    {
                        put(g, value);
                    }
                });
                for entry in &cell.entries {
                    if let Some(shoe) = shoes.get(entry.shoe()) {
                        raster(entry, shoe, law, clip, &mut put);
                    }
                }
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.count
    }
}

/// The one near-grid raster law, shared by live stamps and history checkpoints. It samples
/// global texel centres, so clipping a draw into cells or recentre strips cannot change it.
fn raster(
    entry: &StampEntry,
    shoe: &ClearanceStamp,
    law: &PrintLaw,
    clip: Span,
    mut put: impl FnMut(IVec2, [u8; 2]),
) {
    let (low, high) = entry.world_bounds(shoe);
    let Some(span) = Span::covering(low, high, NEAR_TEXEL_M).intersect(clip) else {
        return;
    };
    let axes = entry.axes();
    for j in span.min.y..span.max.y {
        for i in span.min.x..span.max.x {
            let g = IVec2::new(i, j);
            let world = (g.as_vec2() + 0.5) * NEAR_TEXEL_M;
            let Some((clearance, footprint)) = entry.at(shoe, world, law, axes) else {
                continue;
            };
            let value = [
                press(clearance, law.range_m),
                if footprint { 255 } else { 0 },
            ];
            if value != [0, 0] {
                put(g, value);
            }
        }
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
        if let Some(shoe) = shoes.get(entry.shoe()) {
            raster(entry, shoe, law, clip, |g, value| self.merge(g, value));
        }
    }

    fn merge(&mut self, g: IVec2, value: [u8; 2]) {
        let (index, storage) = Self::slot(g);
        let texel = &mut self.texels[index];
        let merged = [texel[0].max(value[0]), texel[1].max(value[1])];
        if merged != *texel {
            *texel = merged;
            self.mark(storage);
        }
    }

    /// Clear `span` (inside the window) and restore its exact raster from spatial history.
    fn redraw(&mut self, span: Span, log: &StampLog, shoes: &[ClearanceStamp], law: &PrintLaw) {
        for j in span.min.y..span.max.y {
            for i in span.min.x..span.max.x {
                let (index, storage) = Self::slot(IVec2::new(i, j));
                self.texels[index] = [0, 0];
                self.mark(storage);
            }
        }
        log.draw(span, shoes, law, |g, value| self.merge(g, value));
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
    /// Level 0: the 8 × 8 footprint samples of each texel, one bit each. Tiles are
    /// allocated only when printed; empty map area costs one optional pointer per tile.
    masks: Vec<Option<Box<[u64]>>>,
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
            masks: vec![None; tiles * tiles],
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
            let tiles = self.side.div_ceil(TILE);
            let bits = self.masks[((j / TILE) * tiles + i / TILE) as usize]
                .as_ref()
                .map_or(0, |tile| tile[((j % TILE) * TILE + i % TILE) as usize]);
            ((bits.count_ones() * 255 + 32) / 64) as u8
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
        let sample_m = self.texel_m / WORLD_SAMPLES as f32;
        let Some(span) = Span::covering(low - self.origin_m, high - self.origin_m, sample_m)
            .intersect(Span {
                min: IVec2::ZERO,
                max: IVec2::splat((self.side * WORLD_SAMPLES) as i32),
            })
        else {
            return;
        };
        let axes = entry.axes();
        let tiles = self.side.div_ceil(TILE);
        for sj in span.min.y..span.max.y {
            for si in span.min.x..span.max.x {
                let world = self.origin_m + (Vec2::new(si as f32, sj as f32) + 0.5) * sample_m;
                if !entry
                    .at(shoe, world, law, axes)
                    .is_some_and(|(_, footprint)| footprint)
                {
                    continue;
                }
                let (si, sj) = (si as u32, sj as u32);
                let (i, j) = (si / WORLD_SAMPLES, sj / WORLD_SAMPLES);
                let tile_index = ((j / TILE) * tiles + i / TILE) as usize;
                let tile = self.masks[tile_index]
                    .get_or_insert_with(|| vec![0; (TILE * TILE) as usize].into_boxed_slice());
                let mask = &mut tile[((j % TILE) * TILE + i % TILE) as usize];
                let bit = 1_u64 << ((sj % WORLD_SAMPLES) * WORLD_SAMPLES + si % WORLD_SAMPLES);
                if *mask & bit == 0 {
                    *mask |= bit;
                    self.dirty[tile_index] = true;
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
            let entry = StampEntry::new(
                contact.datum,
                contact.yaw,
                contact.lift,
                contact.tilt,
                0,
                false,
            );
            let edge = Vec2::new(-angle.signum() * (bounds.1.x - 0.01), 0.0);
            assert!(
                entry.plane(edge).abs() < LAW.contact_m,
                "the encoded {degrees} degree contact retains its grounded edge"
            );
            assert!(
                entry
                    .at(&shoe, edge, &LAW, entry.axes())
                    .is_some_and(|(_, footprint)| footprint)
            );
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
                log.push(entry, &shoes, &LAW);
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

    /// Recentre relies on exact history even after repeated pivots. Checkpointing must bound
    /// pending operations per cell without losing older pixels, including across negative cell
    /// boundaries; an immortal entry vector or replaying only the latest batch fails here.
    #[test]
    fn compacted_history_is_spatially_bounded_and_replays_every_texel() {
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        let mut log = StampLog::default();
        let mut direct = NearStore::default();
        direct.recentre(Vec2::ZERO, &log, &shoes, &LAW);
        for entry in
            scatter(Vec2::splat(-6.0), 1.0, 4096, 3)
                .into_iter()
                .chain(std::iter::repeat_n(
                    StampEntry::new(Vec2::splat(-6.0), 0.4, 0.0, Vec2::ZERO, 0, false),
                    8192,
                ))
        {
            log.push(entry, &shoes, &LAW);
            direct.stamp(&entry, &shoes, &LAW);
        }
        assert!(
            log.cells
                .values()
                .any(|cell| !matches!(cell.checkpoint, Checkpoint::Empty))
        );
        assert!(
            log.cells
                .values()
                .all(|cell| cell.entries.len() < LOG_BATCH)
        );
        let mut restored = NearStore::default();
        restored.recentre(Vec2::ZERO, &log, &shoes, &LAW);
        assert_eq!(
            direct.texels, restored.texels,
            "compaction preserves all historical near pixels"
        );
        restored.recentre(Vec2::splat(200.0), &log, &shoes, &LAW);
        restored.recentre(Vec2::ZERO, &log, &shoes, &LAW);
        assert_eq!(
            direct.texels, restored.texels,
            "the torus restores compacted history after a full eviction"
        );
        let noisy: Vec<_> = (0..LOG_CELL_SIDE * LOG_CELL_SIDE)
            .map(|i| [(i % 2) as u8, 255])
            .collect();
        assert!(
            matches!(Checkpoint::packed(noisy), Checkpoint::Raw(_)),
            "incompressible checkpoints cannot grow beyond raw cell storage"
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

    /// World coverage must retain a sole visible in the near store at every sample phase.
    /// Four samples per world texel can miss a complete shoe, making it vanish at the seam.
    #[test]
    fn the_world_layer_retains_a_footprint_between_coarse_samples() {
        let shoes = vec![ClearanceStamp::pooled(&sole(), NEAR_TEXEL_M)];
        let mut world = WorldStore::new(1500.0, WORLD_SIDE);
        for phase in 0..8 {
            let datum = Vec2::new(phase as f32 * 2.0, phase as f32 * world.texel_m / 8.0);
            let entry = StampEntry::new(datum, 0.0, 0.0, Vec2::ZERO, 0, false);
            world.stamp(&entry, &shoes, &LAW);
            let cell = ((datum - world.origin_m) / world.texel_m)
                .floor()
                .as_uvec2();
            let covered: u32 = (cell.y - 1..=cell.y + 1)
                .flat_map(|j| (cell.x - 2..=cell.x + 2).map(move |i| (i, j)))
                .map(|(i, j)| u32::from(world.coverage(0, i, j)))
                .sum();
            assert!(
                covered > 0,
                "phase {phase}: a grounded shoe cannot disappear in the world layer"
            );
        }
        let uploads = world.take_uploads();
        assert!(!uploads.is_empty());
        let before: Vec<_> = world.masks.clone();
        world.stamp(
            &StampEntry::new(Vec2::ZERO, 0.0, 0.0, Vec2::ZERO, 0, false),
            &shoes,
            &LAW,
        );
        assert_eq!(
            world.masks, before,
            "overprinting is union, not accumulated darkness"
        );
        assert!(world.take_uploads().is_empty());
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
