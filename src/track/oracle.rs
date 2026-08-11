//! Terrain oracle — the boundary between the track model and whatever the world's ground is
//! made of (architecture §5).
//!
//! The trait is deliberately scalar-and-minimal today: `depth_along` is the one query every
//! consumer (belt physics, wheel articulation, the wrap view's terrain conform) actually makes,
//! and the sandbox proved its semantics (exact analytic first hit, C0, deterministic). The
//! architecture doc records the growth path — batched probes, hit normals, surface material,
//! `covered` for streamed terrain — to be added WITH their first consumer, not before.
//!
//! [`BlockField`] is the one implementation, a min-fold of two EXACT analytic terms:
//!
//! - the **heightmap ground** ([`HeightGrid::cast_ray`]) — the shipped surface on the product
//!   map, a closed-form ray-vs-triangle first hit on the same triangulation the parry collider
//!   uses, with no sampling rate to outrun;
//! - the **authored blocks** — the union of rounded boxes built from the same transforms that
//!   spawn the terrain colliders (the sandbox's step-19..25 field, generalized), which is the
//!   whole surface in the sandbox and the obstacle layer on the map.
//!
//! An Avian `SpatialQuery` adapter for non-block geometry is a per-system construction (the
//! query is a borrowed `SystemParam` and cannot live in a resource) and would land with its
//! first consumer; nothing has needed one.

use bevy::math::{Mat3, Quat, Vec2, Vec3};

use crate::terrain_grid::HeightGrid;

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
/// box, so the BLOCK term is C1 across box edges at the cost of visually-invisible 3 cm corner
/// rounding. Must stay below the smallest authored half-extent. (The heightmap term is only
/// C0 — it is the collider's triangulation exactly, and matching the collider outranks
/// smoothness there.)
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

    /// World-space AABB of the block (this field's broadphase bounds, and the spawn selector's
    /// building footprint): extent along each world axis is the rotated half-extent's projection
    /// sum. [`FIELD_BURY`] moves the bottom only while the block's local +Y points up (every
    /// authored block): the centre shift and the half-extent growth cancel along world Y, so
    /// `max.y` is the authored top.
    pub(crate) fn world_aabb(&self) -> (Vec3, Vec3) {
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
    /// The heightmap ground term: when present, the ground is the piecewise-triangular surface
    /// `h(x, z)` — the SAME surface the collider and render mesh carry (the ONE-SURFACE
    /// invariant, `terrain_grid` module doc) — instead of the flat-slab block the old world
    /// authored. Blocks still union on top.
    height: Option<HeightGrid>,
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
            height: None,
        }
    }

    /// Attach (or clear) the heightmap ground term. Builder-style so the sandbox's existing
    /// `BlockField::new(..)` call sites stay valid without a signature change.
    pub fn with_height(mut self, height: Option<HeightGrid>) -> Self {
        self.height = height;
        self
    }

    /// Dump the exact constructed oracle, including derived transforms and broadphase layout.
    #[cfg(feature = "bitprobe")]
    pub(crate) fn bitprobe_startup(&self, out: &mut crate::bitprobe::StartupBuilder) {
        out.f32("oracle.FIELD_ROUNDING", FIELD_ROUNDING);
        out.f32("oracle.FIELD_BURY", FIELD_BURY);
        out.f32("oracle.FIELD_CELL", FIELD_CELL);
        out.f32("oracle.z0", self.z0);
        out.f32("oracle.cell", self.cell);
        out.u32("oracle.block_count", self.blocks.len() as u32);
        for (index, block) in self.blocks.iter().enumerate() {
            out.vec3(&format!("oracle.blocks[{index}].center"), block.center);
            out.quat(&format!("oracle.blocks[{index}].inv_rot"), block.inv_rot);
            out.vec3(&format!("oracle.blocks[{index}].half"), block.half);
            out.vec3(
                &format!("oracle.blocks[{index}].bounds_lo"),
                self.bounds[index].0,
            );
            out.vec3(
                &format!("oracle.blocks[{index}].bounds_hi"),
                self.bounds[index].1,
            );
        }
        out.u32("oracle.height.present", u32::from(self.height.is_some()));
        if let Some(grid) = &self.height {
            out.u32("oracle.height.size", grid.size());
            out.u32("oracle.height.byte_sum", grid.byte_sum());
            out.f32("oracle.height.world_size", grid.world_size());
            out.f32("oracle.height.offset", grid.extent().height_offset_m);
            out.f32("oracle.height.span", grid.extent().height_span_m);
        }
        out.u32("oracle.grid.bucket_count", self.grid.len() as u32);
        for (bucket, indices) in self.grid.iter().enumerate() {
            out.u32(&format!("oracle.grid[{bucket}].len"), indices.len() as u32);
            for (slot, index) in indices.iter().copied().enumerate() {
                out.u32(&format!("oracle.grid[{bucket}][{slot}]"), u32::from(index));
            }
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
    /// Diagnostic/authoring surface with no live caller: every consumer wants the directional
    /// first hit, so the hot path is [`TerrainOracle::depth_along`].
    pub fn sdf(&self, p: Vec3) -> f32 {
        let blocks = self
            .blocks
            .iter()
            .map(|b| block_sdf(p, b))
            .fold(f32::INFINITY, f32::min);
        match &self.height {
            // Vertical distance to the height surface: exact on flat ground, a conservative
            // bound on slopes — adequate for this fn's diagnostic role (hot paths ray-cast via
            // `depth_along`).
            Some(grid) => blocks.min(p.y - grid.height_at(p.x, p.z)),
            None => blocks,
        }
    }

    /// Signed EUCLIDEAN penetration of `p` (nearest-surface distance, capped at `reach`):
    /// positive inside. Diagnostic use — Euclidean depth under a raised block plateaus at the
    /// block's side-face distance, which is why the physics reads `depth_along`.
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
        // The heightmap ground term: the EXACT first-hit caster (`HeightGrid::cast_ray` — a 2-D
        // DDA over the 1–2 cells a suspension probe's XZ footprint crosses, closed-form
        // ray-vs-triangle per cell on the same anti-diagonal split the collider uses).
        // Deterministic: fixed iteration order, pure f32 arithmetic, no transcendentals. It
        // replaced a fixed 8-segment sign-change scan + 24-step bisection, whose 12.5 cm
        // checkpoints could straddle a thin ridge the ray pierces AND exits between two
        // checkpoints (no sign change at any checkpoint = the crossing was invisible — see the
        // ridge-graze test below); the closed-form crossing has no resolution to slip through.
        // Composes with the blocks by the same min-entry union rule.
        //
        // f(0) ≤ 0 saturation: an origin under the surface saturates to the reach, exactly like
        // a buried block origin (a contact cast) — but only WITHIN the grid span. Outside it the
        // world ends at the collider edge (`HeightGrid::cast_ray` doc): the height term reports
        // no ground at all, so the oracle agrees with the parry collider instead of feeling the
        // clamped phantom ground `height_at` extends for placement queries.
        if let Some(grid) = &self.height {
            if grid.contains_xz(origin.x, origin.z)
                && origin.y <= grid.height_at(origin.x, origin.z)
            {
                buried = true;
            } else if let Some(hit) = grid.cast_ray(origin, out, t_max) {
                t = t.min(hit.t);
            }
        }
        if buried {
            return reach;
        }
        reach - t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain_grid::FIXTURE_EXTENT;

    fn slab() -> BlockField {
        // Ground slab: top face at y = 0 (the world.rs idiom).
        BlockField::new(vec![TerrainBlock::new(
            Vec3::new(0.0, -0.5, 0.0),
            Quat::IDENTITY,
            Vec3::new(100.0, 1.0, 100.0),
        )])
    }

    #[test]
    fn depth_along_reads_clearance_and_penetration_exactly() {
        let field = slab();
        let down = Vec3::NEG_Y;
        // 10 cm above the surface: 10 cm of clearance.
        let d = field.depth_along(Vec3::new(0.0, 0.10, 0.0), down, 0.5);
        assert!((d + 0.10).abs() < 1e-3, "clearance read {d}");
        // 5 cm past the surface: 5 cm of penetration.
        let d = field.depth_along(Vec3::new(0.0, -0.05, 0.0), down, 0.5);
        assert!((d - 0.05).abs() < 1e-3, "penetration read {d}");
        // Buried origin (station deep inside): saturates to the reach.
        let d = field.depth_along(Vec3::new(0.0, -0.9, 0.0), down, 0.5);
        assert_eq!(d, 0.5, "buried probe must saturate");
    }

    /// A height grid whose samples come from `f(i, j)` in 8-bit terms (normalized to meters
    /// like the decoder; see `terrain_grid` for the mapping), hung at the fixture extent.
    fn height_grid(size: u32, f: impl Fn(u32, u32) -> u8) -> HeightGrid {
        let mut samples = Vec::with_capacity((size * size) as usize);
        for j in 0..size {
            for i in 0..size {
                samples.push(f32::from(f(i, j)) * (SPAN / 255.0));
            }
        }
        HeightGrid::new(samples.into(), size, FIXTURE_EXTENT)
    }

    /// The fixture world every synthetic grid below is hung at, and its two derived numbers.
    const SPAN: f32 = FIXTURE_EXTENT.height_span_m;
    const HALF: f32 = FIXTURE_EXTENT.half_extent();

    #[test]
    fn height_term_reads_flat_ground_like_the_slab() {
        // All samples 51 → a flat surface at 51/255 · SPAN = 0.2 · SPAN.
        let h = 51.0 / 255.0 * SPAN;
        let field = BlockField::new(vec![]).with_height(Some(height_grid(4, |_, _| 51)));
        let down = Vec3::NEG_Y;
        // 10 cm above the surface: 10 cm of clearance.
        let d = field.depth_along(Vec3::new(3.0, h + 0.10, -7.0), down, 0.5);
        assert!((d + 0.10).abs() < 1e-3, "clearance read {d}");
        // 5 cm past the surface: 5 cm of penetration.
        let d = field.depth_along(Vec3::new(-20.0, h - 0.05, 11.0), down, 0.5);
        assert!((d - 0.05).abs() < 1e-3, "penetration read {d}");
        // Buried origin (station deep under the surface): saturates to the reach.
        let d = field.depth_along(Vec3::new(0.0, h - 0.9, 0.0), down, 0.5);
        assert_eq!(d, 0.5, "buried probe must saturate");
        // Far above: full clearance floor (reach − t_max = −reach).
        let d = field.depth_along(Vec3::new(0.0, h + 50.0, 0.0), down, 0.5);
        assert_eq!(d, -0.5);
    }

    #[test]
    fn height_term_reads_a_known_slope_exactly() {
        // A pure x-ramp: h(x) = (x + HALF) / (2·HALF) · SPAN, linear — the triangular
        // surface IS the plane, so a vertical probe must read h(x) − station.y exactly.
        let field = BlockField::new(vec![])
            .with_height(Some(height_grid(2, |i, _| if i == 1 { 255 } else { 0 })));
        let down = Vec3::NEG_Y;
        // Probe points as FRACTIONS of the half-extent, so re-scaling the world keeps every
        // sample inside the map instead of silently landing on the edge clamp.
        for frac in [-0.7_f32, -0.08, 0.0, 0.26, 0.94] {
            let x = frac * HALF;
            let h = (x + HALF) / (2.0 * HALF) * SPAN;
            let d = field.depth_along(Vec3::new(x, h - 0.07, 5.0), down, 0.5);
            assert!(
                (d - 0.07).abs() < 1e-3,
                "slope penetration at x={x}: read {d}"
            );
            let d = field.depth_along(Vec3::new(x, h + 0.12, -5.0), down, 0.5);
            assert!(
                (d + 0.12).abs() < 1e-3,
                "slope clearance at x={x}: read {d}"
            );
        }
    }

    /// The steep-slope case the OLD 3-step reprojection intersect could not solve: on a 45°
    /// slope probed along the slope NORMAL, the reprojection map t ← (h(xz(t)) − o.y)/o_y has
    /// derivative exactly −1 — it oscillates between two points forever and returns whatever
    /// the third iterate happened to be (analytic divergence factor −1). The exact caster
    /// (`HeightGrid::cast_ray`, which replaced the interim bracketed bisection) must read the
    /// exact plane distance.
    #[test]
    fn slope_normal_probe_on_a_45_degree_slope_reads_exact_depth() {
        // h(x) = x: a 45° plane through the origin. `HeightGrid::new` takes raw meters, so the
        // map's declared vertical range does not constrain a test grid.
        let size = 3u32;
        let mut samples = Vec::with_capacity((size * size) as usize);
        for _j in 0..size {
            for i in 0..size {
                samples.push(-HALF + i as f32 * HALF);
            }
        }
        let field = BlockField::new(vec![]).with_height(Some(HeightGrid::new(
            samples.into(),
            size,
            FIXTURE_EXTENT,
        )));
        // Outward probe direction = downhill surface normal, exactly the divergent case.
        let normal = Vec3::new(-1.0, 1.0, 0.0).normalize();
        let out = -normal;
        let surface = Vec3::new(0.0, 0.0, 5.0); // on the plane (h(0) = 0)
        // 7 cm past the surface along the probe: exactly 7 cm of penetration.
        let d = field.depth_along(surface + out * 0.07, out, 0.5);
        assert!((d - 0.07).abs() < 1e-3, "45° penetration read {d}");
        // 12 cm of clearance.
        let d = field.depth_along(surface - out * 0.12, out, 0.5);
        assert!((d + 0.12).abs() < 1e-3, "45° clearance read {d}");
        // Deeply buried station: saturates like any buried origin.
        let d = field.depth_along(surface + out * 0.9, out, 0.5);
        assert_eq!(d, 0.5, "buried 45° probe must saturate");
        // Far off the slope: full clearance floor.
        let d = field.depth_along(surface - out * 50.0, out, 0.5);
        assert_eq!(d, -0.5);
    }

    /// The ridge-graze case the OLD checkpoint scan MISSED: the 8-segment sign-change scan
    /// sampled f every t_max/8 = 12.5 cm (reach 0.5 m), so a ray that pierces a thin ridge AND
    /// re-emerges between two consecutive checkpoints saw no sign change and reported full
    /// clearance. The exact caster solves the crossing in closed form — no resolution to slip
    /// through.
    #[test]
    fn ridge_graze_between_old_scan_checkpoints_is_caught() {
        // A 45° tent ridge along z: nodes at x = ∓`HALF` and 0, with h = HALF − |x| (raw meters;
        // the map's declared range does not constrain a test grid). Apex h = HALF at x = 0.
        let size = 3u32;
        let mut samples = Vec::with_capacity((size * size) as usize);
        for _j in 0..size {
            for i in 0..size {
                let x = -HALF + i as f32 * HALF;
                samples.push(HALF - x.abs());
            }
        }
        let field = BlockField::new(vec![]).with_height(Some(HeightGrid::new(
            samples.into(),
            size,
            FIXTURE_EXTENT,
        )));
        // A horizontal +X probe 4 cm below the apex: inside the ridge only for x ∈ (−0.04, 0.04)
        // — 8 cm of ray. Station at x = 0.3, reach 0.5 ⇒ ray origin x = −0.2, so the old scan's
        // checkpoints sat at x = −0.075 and +0.05: the penetration interval lies strictly
        // BETWEEN them. Verify that claim against the surface itself: every old checkpoint reads
        // positive clearance.
        let reach = 0.5_f32;
        let out = Vec3::X;
        let station = Vec3::new(0.3, HALF - 0.04, 7.0);
        let origin = station - out * reach;
        for k in 1..=8 {
            let p = origin + out * (2.0 * reach) * (k as f32 / 8.0);
            let h = HALF - p.x.abs();
            assert!(
                p.y - h > 0.0,
                "checkpoint {k} at x={} must sit clear of the ridge (old-scan miss premise)",
                p.x
            );
        }
        // Exact caster: first crossing at x = −0.04 ⇒ t = 0.16 ⇒ depth = reach − t = 0.34.
        let d = field.depth_along(station, out, reach);
        assert!(
            (d - 0.34).abs() < 1e-3,
            "ridge graze must read 0.34 m past the entry face, got {d}"
        );
    }

    /// THE MAP-EDGE CONSISTENCY HOLE (fixed): outside the grid span the collider ends, so the
    /// oracle's height term must report NO ground — not the clamped flat phantom `height_at`
    /// extends for placement queries (belts feeling ground the hull would fall through).
    #[test]
    fn beyond_the_map_edge_the_height_term_reports_no_ground() {
        // Flat surface at 0.2 · SPAN across the whole span.
        let h = 51.0 / 255.0 * SPAN;
        let field = BlockField::new(vec![]).with_height(Some(height_grid(4, |_, _| 51)));
        let down = Vec3::NEG_Y;
        let outside = HALF + 10.0;
        // A probe 5 cm "under" the clamped phantom surface: no ground, full clearance.
        let d = field.depth_along(Vec3::new(outside, h - 0.05, 0.0), down, 0.5);
        assert_eq!(d, -0.5, "outside the span there is no surface to penetrate");
        // Even deep below the phantom: NOT saturated-buried.
        let d = field.depth_along(Vec3::new(0.0, h - 50.0, -outside), down, 0.5);
        assert_eq!(d, -0.5, "outside the span nothing is buried");
        // Just inside the edge the surface still answers exactly.
        let inside = HALF - 1.0;
        let d = field.depth_along(Vec3::new(inside, h - 0.05, 0.0), down, 0.5);
        assert!((d - 0.05).abs() < 1e-3, "inside the edge reads {d}");
    }

    #[test]
    fn blocks_still_union_on_top_of_the_height_term() {
        // Flat height ground at 0.2·40 = 8 m, plus a block whose top sits 1 m above it: a probe
        // over the block reads the block's top; a probe beside it reads the ground.
        let ground = 51.0 / 255.0 * SPAN;
        let field = BlockField::new(vec![TerrainBlock::new(
            Vec3::new(0.0, ground + 0.5, 0.0),
            Quat::IDENTITY,
            Vec3::new(4.0, 1.0, 4.0),
        )])
        .with_height(Some(height_grid(4, |_, _| 51)));
        let down = Vec3::NEG_Y;
        // 10 cm above the block top (ground + 1): reads clearance to the BLOCK, not the ground.
        let d = field.depth_along(Vec3::new(0.0, ground + 1.10, 0.0), down, 0.5);
        assert!((d + 0.10).abs() < 2e-2, "block-top clearance read {d}");
        // Beside the block: the ground term answers.
        let d = field.depth_along(Vec3::new(30.0, ground - 0.05, 0.0), down, 0.5);
        assert!((d - 0.05).abs() < 1e-3, "ground penetration read {d}");
    }

    #[test]
    fn broadphase_miss_is_full_clearance() {
        // A small block far away; a probe elsewhere has no candidates and must read the same
        // deep-clearance answer an exhaustive miss would.
        let field = BlockField::new(vec![TerrainBlock::new(
            Vec3::new(0.0, 0.0, -50.0),
            Quat::IDENTITY,
            Vec3::splat(1.0),
        )]);
        let d = field.depth_along(Vec3::new(0.0, 1.0, 50.0), Vec3::NEG_Y, 0.5);
        assert_eq!(d, -0.5);
    }
}
