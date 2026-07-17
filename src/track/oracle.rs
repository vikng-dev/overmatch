//! Terrain oracle — the boundary between the track model and whatever the world's ground is
//! made of (architecture §5).
//!
//! The trait is deliberately scalar-and-minimal today: `depth_along` is the one query every
//! consumer (belt physics, chain contacts, wheel articulation, route conform) actually makes,
//! and the sandbox proved its semantics (exact analytic first hit, C0, deterministic). The
//! architecture doc records the growth path — batched `sample_into`, hit normals, surface
//! material, `covered` for streamed terrain — to be added WITH their first consumer, not
//! before.
//!
//! [`BlockField`] is the default implementation: the union of authored rounded boxes, built
//! from the same transforms that spawn the terrain colliders (the sandbox's step-19..25 field,
//! generalized). An Avian `SpatialQuery` adapter for non-block geometry is a per-system
//! construction (the query is a borrowed `SystemParam` and cannot live in a resource) and lands
//! with its first consumer.

use bevy::math::{Mat3, Quat, Vec2, Vec3};

/// A terrain query surface for track consumers. Implementations must be pose-continuous and
/// deterministic (fixed evaluation order, pure arithmetic) — the belt physics samples this.
pub trait TerrainOracle {
    /// Signed directional penetration of `station` past the first terrain surface along `out`:
    /// the ray starts `reach` behind the station and may report at most `reach` of depth
    /// (buried origin saturates to `reach`, like a contact cast). Positive = past the surface,
    /// negative = clearance.
    fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32;
}

/// Edge rounding radius (m) of the block field: every authored box is evaluated as a rounded
/// box, so the union's surface is C1 across box edges at the cost of visually-invisible 3 cm
/// corner rounding. Must stay below the smallest authored half-extent.
pub const FIELD_ROUNDING: f32 = 0.03;

/// How far (m) every block's bottom is extended below its authored extent, so a raised block
/// resting on other geometry carries no interior union seam (depth below a top face grows
/// monotonically instead of collapsing past mid-height — the step-19 "washboard ignored" bug).
pub const FIELD_BURY: f32 = 2.0;

/// Z-extent (m) of one broadphase bucket.
const FIELD_CELL: f32 = 4.0;

/// One authored terrain block (world-space oriented box, bottom extended by [`FIELD_BURY`]).
pub struct TerrainBlock {
    center: Vec3,
    /// World→box rotation (the block's rotation inverted).
    inv_rot: Quat,
    half: Vec3,
}

impl TerrainBlock {
    /// Build from an authored block's world transform: a unit cube at `translation`, rotated by
    /// `rotation`, scaled by `scale`; the bottom extended by [`FIELD_BURY`] along the block's
    /// local −Y (the top surface is untouched).
    pub fn new(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self {
            center: translation - rotation * Vec3::Y * (FIELD_BURY / 2.0),
            inv_rot: rotation.inverse(),
            half: scale / 2.0 + Vec3::Y * (FIELD_BURY / 2.0),
        }
    }

    /// World-space AABB of the block (broadphase bounds): extent along each world axis is the
    /// rotated half-extent's projection sum.
    fn world_aabb(&self) -> (Vec3, Vec3) {
        let m = Mat3::from_quat(self.inv_rot.inverse());
        let ext = m.x_axis.abs() * self.half.x
            + m.y_axis.abs() * self.half.y
            + m.z_axis.abs() * self.half.z;
        (self.center - ext, self.center + ext)
    }

    /// Exact first-hit distance (t ≥ 0) of a ray with this ROUNDED box, or `None` on a miss.
    /// The rounded box is the Minkowski sum of the shrunken core and a [`FIELD_ROUNDING`]
    /// sphere, so its exact surface decomposes into 3 face slabs, 12 edge cylinders, and 8
    /// corner spheres — the union's entry is the min of the primitive entries. Assumes the
    /// origin is outside the box (the caller checks the union's SDF); closed-form quadratics
    /// only, so grazing rays get the exact answer a sphere-trace march could stall on.
    fn ray_hit(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        let r = FIELD_ROUNDING;
        let core = (self.half - Vec3::splat(r)).max(Vec3::splat(1e-3));
        let o = self.inv_rot * (origin - self.center);
        let d = self.inv_rot * dir;

        // Cheap reject: the box inflated by the rounding bounds the whole rounded shape.
        ray_box(o, d, core + Vec3::splat(r))?;

        let mut best = f32::INFINITY;
        // (a) The three face slabs.
        for axis in 0..3 {
            let mut ext = core;
            ext[axis] += r;
            if let Some(t) = ray_box(o, d, ext) {
                best = best.min(t);
            }
        }
        // (b) The twelve edge cylinders: radius r around each core edge, hits accepted only
        // within the edge's axial extent (entries through a cylinder's end cap are inside the
        // corner sphere that covers it, so caps need no test of their own).
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for su in [-1.0_f32, 1.0] {
                for sv in [-1.0_f32, 1.0] {
                    let oc = Vec2::new(o[u] - su * core[u], o[v] - sv * core[v]);
                    let dc = Vec2::new(d[u], d[v]);
                    if let Some(t) = ray_circle(oc, dc, r)
                        && (o[axis] + d[axis] * t).abs() <= core[axis]
                    {
                        best = best.min(t);
                    }
                }
            }
        }
        // (c) The eight corner spheres.
        for sx in [-1.0_f32, 1.0] {
            for sy in [-1.0_f32, 1.0] {
                for sz in [-1.0_f32, 1.0] {
                    let c = Vec3::new(sx * core.x, sy * core.y, sz * core.z);
                    if let Some(t) = ray_sphere(o - c, d, r) {
                        best = best.min(t);
                    }
                }
            }
        }
        (best < f32::INFINITY).then_some(best)
    }
}

/// Entry distance of a ray into an axis-aligned box of half-extents `ext` (slab test), if it
/// hits at t ≥ 0. An origin inside returns 0.
fn ray_box(o: Vec3, d: Vec3, ext: Vec3) -> Option<f32> {
    let (mut t0, mut t1) = (0.0_f32, f32::INFINITY);
    for axis in 0..3 {
        if d[axis].abs() < 1e-9 {
            if o[axis].abs() > ext[axis] {
                return None;
            }
        } else {
            let inv = 1.0 / d[axis];
            let (ta, tb) = ((-ext[axis] - o[axis]) * inv, (ext[axis] - o[axis]) * inv);
            t0 = t0.max(ta.min(tb));
            t1 = t1.min(ta.max(tb));
            if t0 > t1 {
                return None;
            }
        }
    }
    Some(t0)
}

/// Entry distance of a 2D ray into a circle of radius `r` at the origin, if it enters from
/// OUTSIDE at t ≥ 0. An origin already inside returns `None` — for the edge-cylinder use, such
/// a ray can only enter the finite cylinder through an end cap, which the corner spheres cover.
fn ray_circle(o: Vec2, d: Vec2, r: f32) -> Option<f32> {
    let a = d.length_squared();
    if a < 1e-12 {
        return None;
    }
    let b = o.dot(d);
    let c = o.length_squared() - r * r;
    if c <= 0.0 {
        return None;
    }
    let disc = b * b - a * c;
    if disc < 0.0 {
        return None;
    }
    let t = (-b - disc.sqrt()) / a;
    (t >= 0.0).then_some(t)
}

/// Entry distance of a ray into a sphere of radius `r` at the origin (`o` = ray origin relative
/// to the sphere center), if it enters from outside at t ≥ 0.
fn ray_sphere(o: Vec3, d: Vec3, r: f32) -> Option<f32> {
    let a = d.length_squared();
    if a < 1e-12 {
        return None;
    }
    let b = o.dot(d);
    let c = o.length_squared() - r * r;
    if c <= 0.0 {
        return None;
    }
    let disc = b * b - a * c;
    if disc < 0.0 {
        return None;
    }
    let t = (-b - disc.sqrt()) / a;
    (t >= 0.0).then_some(t)
}

/// Quilez rounded-box SDF: exact distance on faces, rounded by [`FIELD_ROUNDING`] at
/// edges/corners.
fn block_sdf(p: Vec3, b: &TerrainBlock) -> f32 {
    let core = (b.half - Vec3::splat(FIELD_ROUNDING)).max(Vec3::splat(1e-3));
    let q = (b.inv_rot * (p - b.center)).abs() - core;
    q.max(Vec3::ZERO).length() + q.max_element().min(0.0) - FIELD_ROUNDING
}

/// The analytic block-terrain oracle: a union of authored rounded boxes with a z-bucket AABB
/// broadphase (the course/world is laid out along z). Built from the SAME transforms that spawn
/// the terrain colliders, so the two representations cannot drift.
///
/// Note the honesty qualification from the architecture doc: the field rounds corners and
/// buries block bottoms — deliberate contact policy, not representational identity with the
/// collider mesh. "Visual ≡ physics" means every track consumer samples THIS oracle.
#[derive(Default)]
pub struct BlockField {
    blocks: Vec<TerrainBlock>,
    /// Per-block world AABB (min, max).
    bounds: Vec<(Vec3, Vec3)>,
    /// Bucket i covers z ∈ [z0 + i·cell, z0 + (i+1)·cell): indices of blocks overlapping it.
    grid: Vec<Vec<u16>>,
    z0: f32,
    cell: f32,
}

impl BlockField {
    pub fn new(blocks: Vec<TerrainBlock>) -> Self {
        let bounds: Vec<(Vec3, Vec3)> = blocks.iter().map(|b| b.world_aabb()).collect();
        let z0 = bounds
            .iter()
            .map(|(lo, _)| lo.z)
            .fold(f32::INFINITY, f32::min);
        let z1 = bounds
            .iter()
            .map(|(_, hi)| hi.z)
            .fold(f32::NEG_INFINITY, f32::max);
        let cells = if bounds.is_empty() {
            0
        } else {
            ((z1 - z0) / FIELD_CELL).ceil().max(1.0) as usize
        };
        let mut grid = vec![Vec::new(); cells];
        for (i, (lo, hi)) in bounds.iter().enumerate() {
            let a = (((lo.z - z0) / FIELD_CELL) as usize).min(cells.saturating_sub(1));
            let b = (((hi.z - z0) / FIELD_CELL) as usize).min(cells.saturating_sub(1));
            for bucket in &mut grid[a..=b] {
                bucket.push(i as u16);
            }
        }
        Self {
            blocks,
            bounds,
            grid,
            z0,
            cell: FIELD_CELL,
        }
    }

    /// Visit every block whose AABB overlaps `[lo, hi]`, in fixed order, possibly more than
    /// once (callers must be duplicate-tolerant — min-folds are).
    fn candidates(&self, lo: Vec3, hi: Vec3, mut visit: impl FnMut(&TerrainBlock)) {
        if self.grid.is_empty() {
            return;
        }
        let last = self.grid.len() - 1;
        let a = ((((lo.z - self.z0) / self.cell) as isize).clamp(0, last as isize)) as usize;
        let b = ((((hi.z - self.z0) / self.cell) as isize).clamp(0, last as isize)) as usize;
        for bucket in &self.grid[a..=b] {
            for &i in bucket {
                let (blo, bhi) = self.bounds[i as usize];
                if lo.x <= bhi.x
                    && hi.x >= blo.x
                    && lo.y <= bhi.y
                    && hi.y >= blo.y
                    && lo.z <= bhi.z
                    && hi.z >= blo.z
                {
                    visit(&self.blocks[i as usize]);
                }
            }
        }
    }

    /// Signed distance (m) from `p` to the terrain surface: negative inside. Union = min over
    /// blocks; full fold — a correct GLOBAL nearest distance can't be bucket-pruned.
    /// Offline/scan use only; hot paths use [`TerrainOracle::depth_along`].
    pub fn sdf(&self, p: Vec3) -> f32 {
        self.blocks
            .iter()
            .map(|b| block_sdf(p, b))
            .fold(f32::INFINITY, f32::min)
    }

    /// Signed EUCLIDEAN penetration of `p` (nearest-surface distance, capped at `reach`):
    /// positive inside. Scan/diagnostic use — Euclidean depth under a raised block plateaus at
    /// the block's side-face distance, which is why the physics reads `depth_along`.
    pub fn signed_depth(&self, p: Vec3, reach: f32) -> f32 {
        (-self.sdf(p)).min(reach)
    }
}

impl TerrainOracle for BlockField {
    fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
        // Anything past one reach beyond the station is deep clearance — contact profiles only
        // need the sign + slope there.
        let t_max = 2.0 * reach;
        let origin = station - out * reach;
        let end = origin + out * t_max;
        let (lo, hi) = (origin.min(end), origin.max(end));
        let mut t = t_max;
        let mut buried = false;
        self.candidates(lo, hi, |b| {
            // A buried origin (inside any block) is fully saturated, like a contact cast. The
            // origin lies inside the probe segment's AABB, so its block is always a candidate.
            buried = buried || block_sdf(origin, b) <= 0.0;
            if !buried && let Some(hit) = b.ray_hit(origin, out) {
                t = t.min(hit);
            }
        });
        if buried {
            return reach;
        }
        reach - t
    }
}
