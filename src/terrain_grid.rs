//! The world height grid: the heightmap the map's manifest names ([`crate::map`]), decoded
//! synchronously at startup (ADR-0014 — sim construction never waits on the async asset server)
//! into a shared, immutable sample slab that every terrain representation derives from:
//!
//! * the track oracle's ground term ([`crate::track::oracle::BlockField`] — the surface the
//!   suspension's `depth_along` probes),
//! * the one static Avian heightfield collider (`world::spawn_environment` — camera/aim rays,
//!   hull contact),
//! * the client's render mesh (same spawn, windowed compositions only),
//! * server spawn placement ([`HeightGrid::height_at`] / [`HeightGrid::max_height_in_square`]).
//!
//! # THE ONE-SURFACE INVARIANT
//!
//! Decode once, downsample once to at most [`GRID_RESOLUTION`]², and publish that [`HeightGrid`].
//! [`HeightGrid::height_at`], [`heightfield_collider`] and [`terrain_mesh_tiles`] use its samples
//! and parry's anti-diagonal cell split, agreeing up to float rounding
//! (`oracle_matches_collider_at_seeded_points`). No consumer sees the full-resolution decode.
//!
//! MEASURED counterexample: separate 4096² bilinear oracle, 1025² triangulated collider and 513²
//! render resamples disagreed by 0.519 m (oracle/collider, beyond the 0.5 m suspension probe) and
//! 1.46 m (oracle/render). The hull could touch ground the belts never felt. Keep one resolution.
//!
//! One decode, one mapping, identical bytes on every peer (the map directory ships in `assets/` on
//! the client archives AND the server tar — see `.github/actions/build-server`), so the
//! deterministic sim reads the same ground everywhere. WHAT the map is — its square, its vertical
//! range, which file it is — is the map's own declaration ([`crate::map::MapManifest`]), not a
//! constant here. Only an explicit [`ForceFlatWorld`] leaves the resource out, and `world` then
//! builds the flat slab + authored test course.

use std::sync::Arc;

use avian3d::prelude::Collider;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;

/// Map-declared horizontal and vertical scale ([`crate::map::MapManifest`]), carried by each
/// [`HeightGrid::extent`]. Oracle, collider, render and spawn mapping all read that owner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainExtent {
    /// Side length (m) of the square world. A map's pixels are STRETCHED over this side,
    /// independently of what the map was authored at.
    pub world_size_m: f32,
    /// Metre height a full-scale-ZERO sample maps to.
    pub height_offset_m: f32,
    /// Metres a full-scale sample adds on top of [`Self::height_offset_m`]. Paired with
    /// [`Self::world_size_m`]: the two together set every slope on the map.
    pub height_span_m: f32,
}

/// Map-declared image-row direction (`crate::map::MapManifest::rows`); columns increase along X.
/// [`grid_from_png`] reverses negative-Z rows once, so all in-memory grids increase toward +Z.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowOrder {
    /// Row 0 at `z = -half`, matching the grid's own mapping — decoded straight through.
    TowardPositiveZ,
    /// Row 0 at `z = +half` — the row order is reversed at decode.
    TowardNegativeZ,
}

impl TerrainExtent {
    /// Half the world side (m): world XZ spans `[-half, +half]`, centered on the origin.
    pub const fn half_extent(&self) -> f32 {
        self.world_size_m / 2.0
    }

    /// Metres per unit of a full-scale-relative sample, and the metre floor to add it to — the
    /// decode's `offset + normalized · span`, pre-divided per bit depth by the caller.
    fn scale_from(&self, full_scale: f32) -> (f32, f32) {
        (self.height_span_m / full_scale, self.height_offset_m)
    }
}

/// FIXTURE extent for the flat slab (`world::GROUND_SIZE`, top at y = 0), analytic ramps and
/// unit fixtures. Shipped maps declare their own extent.
pub(crate) const FIXTURE_EXTENT: TerrainExtent = TerrainExtent {
    world_size_m: 1000.0,
    height_offset_m: 0.0,
    height_span_m: 100.0,
};

/// Grid extent, or the flat slab's when absent, for spawn-map UVs and authority spawn bounds.
pub(crate) fn world_extent(grid: Option<&HeightGrid>) -> TerrainExtent {
    grid.map_or(FIXTURE_EXTENT, HeightGrid::extent)
}

/// Resolution: cap samples per side for parry's cost; the full 4096² decode is too heavy.
/// Every consumer shares this downsample (module invariant). DERIVED spacing: 1500 / 1024 ≈
/// 1.465 m on the shipped world; other map extents change spacing, not sample count.
pub(crate) const GRID_RESOLUTION: u32 = 1025;

/// Resolution: cells per render tile for per-entity frustum culling. DERIVED: the 1025² grid
/// becomes 8×8 tiles of 128² cells instead of drawing ~2.1M triangles every frame.
pub(crate) const MESH_TILE_CELLS: usize = 128;

/// Real-world coverage the ACTIVE pack was authored at, metres per texture repeat. Poly Haven
/// scans declare their physical size (`api.polyhaven.com/info/coast_sand_rocks_02` →
/// `dimensions: [15000, 15000]`, millimetres — the unit is pinned by `rocks_ground_02`, which
/// carries both `dimensions: [2000, 2000]` and the human-readable `scale: "2x2M"`). Recorded in
/// the pack's `cc.txt` as part of its contract.
///
/// Only the BASE pack needs a constant here — the blended layers carry theirs in
/// [`crate::terrain_blend::BLEND_LAYERS`], and apply it as a UV scale in the shader. The two
/// folders left under `assets/terrain/` that neither binds (`brown_mud_leaves_01`,
/// `rocks_ground_02`) are STAGED: nothing loads them, so they carry their `cc.txt` and no maps,
/// which is all `scripts/encode-terrain-ktx2.sh` needs to cut one.
const COAST_SAND_ROCKS_02_AUTHORED_M: f32 = 15.0;

/// Authored world metres per repeat (`world_xz / this`, repeat sampler), not a taste. View-only:
/// texturing never changes grid/oracle/collider, and world rescaling keeps physical texel density.
/// DERIVED counterexample: fitting this 15 m scan into an 8 m guess gives 0.53× features — a 1 m
/// boulder reads as 53 cm, making the terrain look like a scale model.
pub(crate) const TEXTURE_TILE_M: f32 = COAST_SAND_ROCKS_02_AUTHORED_M;

/// The BASE terrain surface pack — Poly Haven `coast_sand_rocks_02`, CC0 (see the pack folder's
/// `cc.txt`), relative to the asset root. One folder per pack under `assets/terrain/`, each holding
/// the three maps `world::spawn_environment` binds: albedo (`diff`/`col`, sRGB), OpenGL-convention
/// tangent-space normals (`nor_gl`), and the glTF-ORM `arm` pack (R = AO, G = roughness,
/// B = metallic). This is what the ground is wherever the map's masks are silent; the three packs
/// blended on top of it ship as 2D ARRAYS under `assets/terrain/blend/`
/// ([`crate::terrain_blend`]), and the two folders neither binds hold only their `cc.txt`.
///
/// Its three maps are **KTX2 (UASTC 4x4, zstd-supercompressed, full mip chain)**, built by
/// `scripts/encode-terrain-ktx2.sh` from masters kept outside the repo. The format is not a
/// packaging detail — it is the frame budget. A PNG/JPG load gives bevy a texture with ONE mip
/// level, and a 4k map tiled every [`TEXTURE_TILE_M`] across the whole horizon then misses the
/// texture cache on nearly every fetch (measured: 30 fps). Mips fix the sampling; the block
/// compression cuts the resident bytes 4× on top. Do not "simplify" these back to PNG.
pub(crate) const TEXTURE_PATH: &str = "terrain/coast_sand_rocks_02/coast_sand_rocks_02_diff.ktx2";
/// Tangent-space normal map of [`TEXTURE_PATH`]'s pack — see it for the pack layout.
pub(crate) const NORMAL_PATH: &str = "terrain/coast_sand_rocks_02/coast_sand_rocks_02_nor_gl.ktx2";
/// AO / roughness / metallic pack of [`TEXTURE_PATH`]'s pack — see it for the channel layout.
pub(crate) const ARM_PATH: &str = "terrain/coast_sand_rocks_02/coast_sand_rocks_02_arm.ktx2";

/// Opt-out marker: insert BEFORE the first update to keep the flat slab + authored test course
/// even when the heightmap PNG is present. Test fixtures (`headless_test`, whose transmission
/// gates drive the authored ramps) and the armor sandbox insert this; product compositions do not.
#[derive(Resource, Default)]
pub struct ForceFlatWorld;

/// Clearance every spawn keeps above the sampled surface, metres — the flat-pad spawn's `y = 2.0`
/// over a surface at 0, reproduced over terrain as the footprint's max ground height plus this.
pub(crate) const SPAWN_CLEARANCE_M: f32 = 2.0;

/// Spawn footprint half-side (m): a fixed axis-aligned square, independent of hull construction or yaw.
/// DERIVED coverage: 10 m × 10 m; larger hulls need a construction-derived footprint.
/// Sample its maximum, not its centre: MEASURED centre-only placement buried uphill axles 1.65 m.
pub(crate) const SPAWN_FOOTPRINT_HALF_M: f32 = 5.0;

/// Shared ground-height rule for all spawns: resolve the horizontal definition at spawn time.
/// Authored Y goes stale when a map is re-authored or rescaled. MEASURED counterexample: fixed
/// `y = 2.0` duel spawns sat ~116 m below terrain. No better constant replaces a surface query.
/// No grid means the flat slab at y = 0; adding clearance reproduces the flat-pad y = 2.0 pose.
pub(crate) fn spawn_surface_height(grid: Option<&HeightGrid>, xz: Vec2) -> f32 {
    grid.map_or(0.0, |grid| {
        // Outside the collider there is no surface; an edge-clamped height would spawn a tank
        // over void. Callers clamp via `net::spawn_map::spawn_limit` or use constant positions,
        // so an out-of-span point here is a code bug (ADR-0011), not client input.
        assert!(
            grid.contains_xz(xz.x, xz.y),
            "spawn at ({}, {}) is outside the ±{} m world span — spawn points must be clamped \
             into the placeable square before they are resolved",
            xz.x,
            xz.y,
            grid.half_extent(),
        );
        grid.max_height_in_square(xz.x, xz.y, SPAWN_FOOTPRINT_HALF_M)
    })
}

/// Horizontal XZ → footprint ground height + [`SPAWN_CLEARANCE_M`].
/// [`spawn_surface_height`] owns why spawn definitions carry no Y.
pub(crate) fn spawn_pos(grid: Option<&HeightGrid>, xz: Vec2) -> Vec3 {
    Vec3::new(
        xz.x,
        spawn_surface_height(grid, xz) + SPAWN_CLEARANCE_M,
        xz.y,
    )
}

/// The decoded height grid: heights in METERS as f32 (row-major, row = z, column = x), already
/// bit-depth-normalized (8-bit `v` → `v / 255`, 16-bit → `v / 65535`, then scaled and offset by the
/// grid's own [`TerrainExtent`]) — every consumer reads this truth, so sim, collider and visuals
/// cannot diverge. Shared via `Arc` so the
/// oracle's copy inside [`crate::track::terrain::TrackField`] is free.
///
/// Mapping (the single convention every consumer uses): sample `(i, j)` sits at
/// `x = -half_extent + i * world_size / (size - 1)` (same for `z` with `j`), so the grid spans
/// `[-half_extent, +half_extent]` inclusive on both axes. Outside the span [`Self::height_at`]
/// clamps (a placement-only convenience) — but THE WORLD ENDS AT THE COLLIDER EDGE: the parry
/// heightfield stops at the span, and so do the surface queries ([`Self::cast_ray`], the track
/// oracle's ground term). See [`Self::contains_xz`].
#[derive(Resource, Clone)]
pub struct HeightGrid {
    samples: Arc<[f32]>,
    /// Samples per side ([`GRID_RESOLUTION`] for the shipped map after the one-time downsample;
    /// tests build smaller grids).
    size: u32,
    /// The square these samples cover and the metres they span — carried rather than looked up, so
    /// every mapping in this module is a function of the grid alone.
    extent: TerrainExtent,
}

/// One terrain-surface hit from [`HeightGrid::cast_ray`]: `t` is the hit parameter along the
/// (not necessarily unit) direction — the hit point is `origin + dir * t` — and `normal` is the
/// struck triangle's unit normal, flipped to face the incoming ray (the same convention a parry
/// raycast reports).
#[derive(Clone, Copy, Debug)]
pub struct TerrainRayHit {
    pub t: f32,
    pub normal: Vec3,
}

impl HeightGrid {
    /// Build from height samples in meters over `extent`; `samples.len()` must be `size * size`
    /// and `size >= 2`.
    pub fn new(samples: Arc<[f32]>, size: u32, extent: TerrainExtent) -> Self {
        assert!(size >= 2, "height grid needs at least 2 samples per side");
        assert_eq!(
            samples.len(),
            (size as usize) * (size as usize),
            "height grid sample count must be size²"
        );
        Self {
            samples,
            size,
            extent,
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn extent(&self) -> TerrainExtent {
        self.extent
    }

    /// World side length (m) this grid spans.
    pub fn world_size(&self) -> f32 {
        self.extent.world_size_m
    }

    /// World half-extent (m) this grid spans, so consumers holding a grid need not reach through
    /// [`Self::extent`].
    pub fn half_extent(&self) -> f32 {
        self.extent.half_extent()
    }

    /// Metres at a node; [`crate::terrain_lod`] reads this same slab rather than resampling.
    pub(crate) fn sample(&self, i: usize, j: usize) -> f32 {
        self.samples[j * self.size as usize + i]
    }

    /// Metres at world `(x, z)`, piecewise triangular and edge-clamped for placement only.
    /// Surface consumers (track oracle, [`Self::cast_ray`]) reject out-of-span points: clamped
    /// phantom ground would support belts where the hull collider falls through.
    ///
    /// Uses parry's `triangles_at` default non-zigzag split: anti-diagonal
    /// `(x_lo, z_hi)`–`(x_hi, z_lo)`, triangles `(p00, p10, p01)` / `(p10, p11, p01)`.
    /// `oracle_matches_collider_at_seeded_points` pins agreement up to float rounding; bilinear
    /// interpolation would break it. Fixed-order arithmetic has no platform-varying operations
    /// because the deterministic belt sim probes this ground. Server spawn/respawn also reads it
    /// (`src/net/server.rs`; see [`Self::max_height_in_square`]).
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let n = self.size as usize;
        let last = (n - 1) as f32;
        let (half, world) = (self.half_extent(), self.world_size());
        let u = ((x + half) / world * last).clamp(0.0, last);
        let v = ((z + half) / world * last).clamp(0.0, last);
        let i0 = (u as usize).min(n - 2);
        let j0 = (v as usize).min(n - 2);
        let fu = u - i0 as f32;
        let fv = v - j0 as f32;
        let h00 = self.sample(i0, j0);
        let h10 = self.sample(i0 + 1, j0);
        let h01 = self.sample(i0, j0 + 1);
        let h11 = self.sample(i0 + 1, j0 + 1);
        // The anti-diagonal split: below/on `fu + fv = 1` the plane through (h00, h10, h01),
        // above it the plane through (h11, h10, h01) — exactly parry's two cell triangles.
        if fu + fv <= 1.0 {
            h00 + (h10 - h00) * fu + (h01 - h00) * fv
        } else {
            h11 + (h10 - h11) * (1.0 - fv) + (h01 - h11) * (1.0 - fu)
        }
    }

    /// Conservative maximum height (m) over the axis-aligned square of half-side `half` at world `(x, z)`.
    /// Spawn placement must clear uphill gear, not only the centre (see [`SPAWN_FOOTPRINT_HALF_M`]).
    /// Take every node of every touched cell: piecewise-linear extrema occur at nodes, so their
    /// enclosing rectangle bounds the square's surface maximum. Pure and deterministic.
    pub fn max_height_in_square(&self, x: f32, z: f32, half: f32) -> f32 {
        let n = self.size as usize;
        let last = (n - 1) as f32;
        let (half_extent, world) = (self.half_extent(), self.world_size());
        let node = |w: f32| ((w + half_extent) / world * last).clamp(0.0, last);
        let i0 = node(x - half).floor() as usize;
        let i1 = (node(x + half).ceil() as usize).min(n - 1);
        let j0 = node(z - half).floor() as usize;
        let j1 = (node(z + half).ceil() as usize).min(n - 1);
        let mut max = f32::NEG_INFINITY;
        for j in j0..=j1 {
            for i in i0..=i1 {
                max = max.max(self.sample(i, j));
            }
        }
        max
    }

    /// Whether `(x, z)` lies on the collider's finite surface. [`Self::height_at`]'s out-of-span
    /// clamp serves placement only; [`Self::cast_ray`] reports no ground there.
    pub fn contains_xz(&self, x: f32, z: f32) -> bool {
        let half = self.half_extent();
        x.abs() <= half && z.abs() <= half
    }

    /// First hit on the two-sided triangular surface within `t ∈ [0, t_max]`, or `None`.
    /// `dir` need not be unit: the point is `origin + dir * t`.
    ///
    /// A deterministic 2-D DDA visits crossed XZ cells in ray order, solving each triangle's linear
    /// gap `f(t) = ray_y(t) − plane(ray_xz(t))` with one division. Fixed order, pure f32, no
    /// transcendentals and a hard iteration bound; no scan spacing to miss a ridge or bisection.
    /// Triangles share [`Self::height_at`]'s anti-diagonal / parry surface. Crossings from either
    /// side report; rays remaining on one side miss.
    ///
    /// No cells exist outside the collider span, unlike placement-only edge clamping. Spawns
    /// stay inside `net::spawn_map::spawn_limit` (DERIVED 95 % of half-extent) as maps rescale.
    pub fn cast_ray(&self, origin: Vec3, dir: Vec3, t_max: f32) -> Option<TerrainRayHit> {
        let n = self.size as usize;
        let last = (n - 1) as f32;
        // Grid-space ray: u(t) = u0 + du·t, v(t) = v0 + dv·t (the exact `height_at` mapping).
        let (half, world) = (self.half_extent(), self.world_size());
        let u0 = (origin.x + half) / world * last;
        let v0 = (origin.z + half) / world * last;
        let du = dir.x / world * last;
        let dv = dir.z / world * last;
        // Clip [0, t_max] against the grid span (slab test per axis): an empty clip means the
        // footprint never crosses the span — no ground anywhere along the ray.
        let (mut t0, mut t1) = (0.0_f32, t_max);
        for (o, d) in [(u0, du), (v0, dv)] {
            if d == 0.0 {
                if !(0.0..=last).contains(&o) {
                    return None;
                }
            } else {
                let (ta, tb) = ((0.0 - o) / d, (last - o) / d);
                t0 = t0.max(ta.min(tb));
                t1 = t1.min(ta.max(tb));
            }
        }
        if t0 > t1 {
            return None;
        }
        // Resolution: pad cell ownership by 0.1 mm of t for unit directions, below consumer
        // tolerances, so float-rounded shared-boundary hits reach both cells; first-in-ray wins.
        // Global `[t0, t1] ⊆ [0, t_max]` stays unpadded: clamping an outside root to an endpoint
        // turns micron clearance into full-reach contact for an away-facing probe.
        const EPS_T: f32 = 1.0e-4;
        let cell = |w: f32| (w as usize).min(n - 2); // `as usize` saturates negatives to 0
        let (mut i, mut j) = (cell(u0 + du * t0), cell(v0 + dv * t0));
        let mut t_in = t0;
        // Fixed bound: each iteration either returns or crosses one cell boundary, and the clip
        // window holds at most (n − 1) boundaries per axis.
        for _ in 0..2 * n {
            // Where the ray leaves this cell along each axis (∞ when it never does).
            let t_u = if du > 0.0 {
                ((i + 1) as f32 - u0) / du
            } else if du < 0.0 {
                (i as f32 - u0) / du
            } else {
                f32::INFINITY
            };
            let t_v = if dv > 0.0 {
                ((j + 1) as f32 - v0) / dv
            } else if dv < 0.0 {
                (j as f32 - v0) / dv
            } else {
                f32::INFINITY
            };
            let t_out = t_u.min(t_v).min(t1);
            if let Some(hit) = self.cell_crossing(
                i,
                j,
                origin.y,
                dir,
                (u0, du),
                (v0, dv),
                t_in - EPS_T,
                t_out + EPS_T,
                (t0, t1),
            ) {
                return Some(hit);
            }
            if t_out >= t1 {
                return None;
            }
            // Step to the neighbouring cell (both axes on an exact corner crossing).
            if t_u <= t_out {
                if du > 0.0 {
                    i += 1;
                } else if i == 0 {
                    return None;
                } else {
                    i -= 1;
                }
            }
            if t_v <= t_out {
                if dv > 0.0 {
                    j += 1;
                } else if j == 0 {
                    return None;
                } else {
                    j -= 1;
                }
            }
            if i > n - 2 || j > n - 2 {
                return None;
            }
            t_in = t_out;
        }
        None
    }

    /// The ray's first crossing of cell `(i, j)`'s two triangles within `t ∈ [t_lo, t_hi]`.
    /// Each triangle's plane makes the surface gap linear in `t` (see [`Self::cast_ray`]); the
    /// diagonal side test picks which plane owns the crossing point, with a hair of tolerance —
    /// both planes contain the shared diagonal, so the overlap band is consistent.
    ///
    /// `[t_lo, t_hi]` is the EPS-padded per-cell window (boundary-ownership slop only);
    /// `hard` is the unpadded global clip `[t0, t1] ⊆ [0, t_max]` — a root outside it is
    /// rejected outright, never clamped onto an endpoint.
    #[expect(
        clippy::too_many_arguments,
        reason = "the cell (i, j), the ray's height, direction and u/v lines, and two windows that \
                  differ on purpose (EPS-padded per cell, unpadded global clip)"
    )]
    fn cell_crossing(
        &self,
        i: usize,
        j: usize,
        y0: f32,
        dir: Vec3,
        (u0, du): (f32, f32),
        (v0, dv): (f32, f32),
        t_lo: f32,
        t_hi: f32,
        hard: (f32, f32),
    ) -> Option<TerrainRayHit> {
        const EPS_DIAG: f32 = 1.0e-5;
        let in_window = |t: f32| t >= t_lo && t <= t_hi && t >= hard.0 && t <= hard.1;
        let scale = (self.size - 1) as f32 / self.world_size(); // cells per world metre (slope units)
        let h00 = self.sample(i, j);
        let h10 = self.sample(i + 1, j);
        let h01 = self.sample(i, j + 1);
        let h11 = self.sample(i + 1, j + 1);
        let fu0 = u0 - i as f32;
        let fv0 = v0 - j as f32;
        let mut best: Option<(f32, Vec3)> = None;
        // Lower triangle {h00, h10, h01}: the plane below/on fu + fv = 1 (height_at's form).
        let (a, b) = (h10 - h00, h01 - h00);
        let df = dir.y - (a * du + b * dv);
        if df != 0.0 {
            let t = -(y0 - (h00 + a * fu0 + b * fv0)) / df;
            if in_window(t) {
                let (fu, fv) = (fu0 + du * t, fv0 + dv * t);
                if fu + fv <= 1.0 + EPS_DIAG {
                    best = Some((t, Vec3::new(-a * scale, 1.0, -b * scale)));
                }
            }
        }
        // Upper triangle {h11, h10, h01}: the plane above/on the diagonal.
        let (c, d) = (h10 - h11, h01 - h11);
        let df = dir.y + c * dv + d * du;
        if df != 0.0 {
            let t = -(y0 - (h11 + c * (1.0 - fv0) + d * (1.0 - fu0))) / df;
            if in_window(t) && best.is_none_or(|(lower, _)| t < lower) {
                let (fu, fv) = (fu0 + du * t, fv0 + dv * t);
                if fu + fv >= 1.0 - EPS_DIAG {
                    best = Some((t, Vec3::new(d * scale, 1.0, c * scale)));
                }
            }
        }
        best.map(|(t, normal)| {
            let normal = normal.normalize();
            TerrainRayHit {
                t,
                normal: if normal.dot(dir) > 0.0 {
                    -normal
                } else {
                    normal
                },
            }
        })
    }

    /// Deterministic content fingerprint for the bitprobe startup dump (exact bit patterns, so
    /// any cross-platform grid divergence shows up here first).
    #[cfg(feature = "bitprobe")]
    pub(crate) fn byte_sum(&self) -> u32 {
        self.samples
            .iter()
            .fold(0u32, |acc, &s| acc.wrapping_add(s.to_bits()))
    }
}

/// Decode synchronously at Startup, before `world::spawn_environment` (chained by `world::plugin`).
/// Except for [`ForceFlatWorld`], compositions require the selected map's valid `level.json` and
/// decodable square heightmap; broken shipped data panics (ADR-0011). CI's
/// `map::tests::shipped_map_ships_a_valid_manifest` guards half-shipped maps before boot.
///
/// Publish the one parsed manifest for `scatter` to share the decode's extent and object placement.
/// Decode [`crate::ground_field::GroundField`] masks beside their heightmap; flat, preset or
/// flattened worlds use only the base pack's ground.
pub(crate) fn decode_height_grid(
    mut commands: Commands,
    flat: Option<Res<ForceFlatWorld>>,
    // Pre-inserted grids are synthetic worlds (driving-feel ramps); shipped decode must not
    // replace them. Product compositions never supply one.
    preset: Option<Res<HeightGrid>>,
    // Likewise a manifest inserted before the first update is an explicit fixture map (the tick
    // cost bed's variants of the shipped placement), read in place of the shipped file.
    preset_manifest: Option<Res<crate::map::MapManifest>>,
) {
    let table = crate::ground_types::GroundTypes::load_shipped();
    if flat.is_some() {
        info!("terrain: ForceFlatWorld set — keeping the flat slab + authored course");
        commands.insert_resource(crate::ground_field::GroundField::unpainted(&table));
        return;
    }
    let root = crate::assets::asset_root();
    let manifest = match preset_manifest {
        Some(preset) => preset.clone(),
        None => crate::map::load(&root),
    };
    let extent = manifest.extent;
    let rows = manifest.rows;
    let path = manifest.heightmap_path();
    let heightmap = manifest.heightmap().to_owned();
    // The map paints the ground only where its own heightmap is the surface: a preset or
    // flattened grid stands on the base pack's ground.
    let ground = if preset.is_some() || crate::lod_showcase::enabled() {
        crate::ground_field::GroundField::unpainted(&table)
    } else {
        crate::ground_field::GroundField::load_painted(&manifest, &root, &table)
    };
    commands.insert_resource(ground);
    // Published before every early return below: a preset or flattened grid still stands on the
    // map's objects, which read their placement out of this resource.
    commands.insert_resource(manifest);
    if let Some(preset) = preset {
        info!(
            "terrain: pre-inserted height grid {size}x{size} — skipping the shipped map decode",
            size = preset.size(),
        );
        return;
    }
    if crate::lod_showcase::enabled() {
        // A zero grid keeps collider, render and oracle level on the map's own extent. Omitting
        // it would add the flat fixture world's obstacle course in front of the showcase;
        // flattening only render would leave tanks on invisible hills.
        let size = GRID_RESOLUTION;
        let samples = vec![0.0_f32; (size as usize) * (size as usize)];
        info!(
            "terrain: LodShowcase scene selected — {size}x{size} grid FLATTENED to y = 0 (oracle, \
             collider and render mesh all from these samples)",
        );
        commands.insert_resource(HeightGrid::new(samples.into(), size, extent));
        return;
    }
    let bytes = std::fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "terrain: {} declares heightmap {heightmap} — {}: {err}",
            crate::map::level_path(&root).display(),
            path.display(),
        )
    });
    let grid = grid_from_png(&bytes, extent, rows)
        .unwrap_or_else(|err| panic!("terrain: {} failed to decode: {err}", path.display()));
    info!(
        "terrain: height grid {size}x{size} decoded ({world} m span, {lo}..{hi} m)",
        size = grid.size(),
        world = extent.world_size_m,
        lo = extent.height_offset_m,
        hi = extent.height_offset_m + extent.height_span_m,
    );
    commands.insert_resource(grid);
}

/// PNG bytes + declared extent and row order → [`HeightGrid`]: decode, bit-depth-normalize
/// and orient. Pure, so tests decode the shipped asset through it (`tests::shipped_grid`); a Git-LFS pointer shipping
/// in place of the map fails `map::tests::shipped_map_ships_a_valid_manifest` in CI instead of at
/// boot.
pub(crate) fn grid_from_png(
    bytes: &[u8],
    extent: TerrainExtent,
    rows: RowOrder,
) -> Result<HeightGrid, String> {
    let image = image::load_from_memory(bytes).map_err(|err| err.to_string())?;
    // Normalize to 0..1 of full scale regardless of source bit depth, then scale and offset to
    // meters — an 8-bit and a 16-bit export of the same map interchange freely.
    let (samples, width, height): (Vec<f32>, u32, u32) = match image {
        image::DynamicImage::ImageLuma8(buf) => {
            let (w, h) = buf.dimensions();
            let (scale, offset) = extent.scale_from(255.0);
            let samples = buf
                .into_raw()
                .into_iter()
                .map(|v| f32::from(v) * scale + offset)
                .collect();
            (samples, w, h)
        }
        image::DynamicImage::ImageLuma16(buf) => {
            let (w, h) = buf.dimensions();
            let (scale, offset) = extent.scale_from(65535.0);
            let samples = buf
                .into_raw()
                .into_iter()
                .map(|v| f32::from(v) * scale + offset)
                .collect();
            (samples, w, h)
        }
        other => {
            let buf = other.into_luma16();
            let (w, h) = buf.dimensions();
            let (scale, offset) = extent.scale_from(65535.0);
            let samples = buf
                .into_raw()
                .into_iter()
                .map(|v| f32::from(v) * scale + offset)
                .collect();
            (samples, w, h)
        }
    };
    if width != height {
        return Err(format!("must be square, got {width}x{height}"));
    }
    // Reverse negative-Z rows before downsampling: row 0 becomes z = -half, in the world's
    // frame. Index remapping preserves decoded sample values bit-for-bit.
    let samples = match rows {
        RowOrder::TowardPositiveZ => samples,
        RowOrder::TowardNegativeZ => flip_rows(&samples, width as usize),
    };
    let grid = HeightGrid::new(samples.into(), width, extent);
    // Publish only this one resample. At-or-below-resolution maps (including tiny tests) pass
    // through unchanged.
    if grid.size() > GRID_RESOLUTION {
        Ok(downsample(&grid, GRID_RESOLUTION))
    } else {
        Ok(grid)
    }
}

/// Reverse a square row-major sample slab's ROW order, moving values without touching them — the
/// `-Z` image's one and only correction (see [`RowOrder`]).
fn flip_rows(samples: &[f32], size: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(samples.len());
    for j in (0..size).rev() {
        out.extend_from_slice(&samples[j * size..(j + 1) * size]);
    }
    out
}

/// Resample `grid` onto a `target`² node lattice over the same world square: each new node reads
/// `height_at` at its own world position (the source's triangular surface — pure f32 arithmetic
/// in a fixed order, so every peer downsamples bit-identically). Runs ONCE at decode; after it,
/// the source resolution no longer exists anywhere.
pub(crate) fn downsample(grid: &HeightGrid, target: u32) -> HeightGrid {
    let n = target as usize;
    let half = grid.half_extent();
    let step = grid.world_size() / (n - 1) as f32;
    let mut samples = Vec::with_capacity(n * n);
    for j in 0..n {
        let z = -half + j as f32 * step;
        for i in 0..n {
            let x = -half + i as f32 * step;
            samples.push(grid.height_at(x, z));
        }
    }
    HeightGrid::new(samples.into(), target, grid.extent())
}

/// Static heightfield over the grid's full `[-half_extent, +half_extent]` square, using its own
/// samples and anti-diagonal split. EMPIRICAL: `oracle_matches_collider_at_seeded_points` guards
/// orientation and millimetre agreement, catching transposes. Avian 0.7's
/// `Collider::heightfield(Vec<Vec<f32>>, scale)` layout is `sample(i, j)` at `data[i·n + j]`;
/// parry's column-major `Array2` reads `heights_zx`: i along X, j along Z. The origin-centred
/// field uses total X/Z spans, with scale.y multiplying raw heights; trust raycasts over prose.
///
/// Set parry's `FIX_INTERNAL_EDGES` (absent from Avian's constructor) to keep contact normals
/// inside the triangle/neighbour cone. Continuous-surface edges then push along the surface,
/// rather than back off the edge (`a_frictionless_edge_slides_across_flat_terrain_at_speed`).
/// Only contact manifolds read the flag; ray, shape and point queries are unchanged.
pub(crate) fn heightfield_collider(grid: &HeightGrid) -> Collider {
    use avian3d::parry::shape::{HeightFieldFlags, SharedShape};
    use avian3d::parry::utils::Array2;

    let n = grid.size() as usize;
    let mut heights = Vec::with_capacity(n * n);
    for i in 0..n {
        for j in 0..n {
            heights.push(grid.sample(i, j));
        }
    }
    let world = grid.world_size();
    Collider::from(SharedShape::heightfield_with_flags(
        Array2::new(n, n, heights),
        Vec3::new(world, 1.0, world),
        HeightFieldFlags::FIX_INTERNAL_EDGES,
    ))
}

/// Render-only storage for every terrain level. Static vertices have no CPU reader: collider
/// and oracle read the grid; pinned `Aabb` / `NoAutoAabb` prevent bounds re-derivation. DERIVED
/// from ~2.9 M triangles and ~1.5 M tangented vertices: the default CPU copy costs ~100 MiB.
/// Future CPU geometry readers must use the grid.
pub(crate) const TERRAIN_MESH_USAGE: RenderAssetUsages = RenderAssetUsages::RENDER_WORLD;

/// Client meshes from exact grid samples and anti-diagonals, chunked into [`MESH_TILE_CELLS`]²
/// tiles for entity culling. World-space positions require `Transform::IDENTITY`; adjacent tiles
/// share border vertices. Normals use full-grid central differences, UVs `world_xz / TEXTURE_TILE_M`
/// with repeat sampling. `world::spawn_environment` builds these only with a window, never on
/// the dedicated server.
pub(crate) fn terrain_mesh_tiles(grid: &HeightGrid) -> Vec<Mesh> {
    let ranges = mesh_tile_node_ranges(grid);
    let mut meshes = Vec::with_capacity(ranges.len());
    for [ia, ib, ja, jb] in ranges {
        let (w, h) = (ib - ia + 1, jb - ja + 1);
        let mut positions: Vec<[f32; 3]> = Vec::with_capacity(w * h);
        let mut normals: Vec<[f32; 3]> = Vec::with_capacity(w * h);
        let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(w * h);
        for j in ja..=jb {
            let z = node_world_coord(grid, j);
            for i in ia..=ib {
                let x = node_world_coord(grid, i);
                positions.push([x, grid.sample(i, j), z]);
                normals.push(surface_normal_at(grid, i, j));
                uvs.push([x / TEXTURE_TILE_M, z / TEXTURE_TILE_M]);
            }
        }
        let mut indices: Vec<u32> = Vec::with_capacity((w - 1) * (h - 1) * 6);
        for j in 0..h - 1 {
            for i in 0..w - 1 {
                let i00 = (j * w + i) as u32;
                let i10 = i00 + 1; // +x
                let i01 = i00 + w as u32; // +z
                let i11 = i01 + 1;
                // Counter-clockwise seen from +Y (up), split along the shared edge
                // i01–i10: the SAME anti-diagonal parry and `height_at` use.
                indices.extend_from_slice(&[i00, i01, i10, i10, i01, i11]);
            }
        }
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, TERRAIN_MESH_USAGE);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(Indices::U32(indices));
        meshes.push(mesh);
    }
    meshes
}

/// World coordinate (m) of grid node index `k` along either axis — the single mapping
/// [`HeightGrid::height_at`] inverts, exposed so the render mesh and the LOD ladder place their
/// vertices at exactly the same points instead of each rebuilding the arithmetic.
pub(crate) fn node_world_coord(grid: &HeightGrid, k: usize) -> f32 {
    -grid.half_extent() + k as f32 * (grid.world_size() / (grid.size() - 1) as f32)
}

/// Full-grid central-difference normal at `(i, j)`, one-sided at map edges. The LOD ladder
/// samples kept nodes here: recomputing from simplified triangles would pop shading with geometry.
/// Reading the grid across tile borders keeps shared normals identical at every level.
pub(crate) fn surface_normal_at(grid: &HeightGrid, i: usize, j: usize) -> [f32; 3] {
    let n = grid.size() as usize;
    let step = grid.world_size() / (n - 1) as f32;
    let (il, ih) = (i.saturating_sub(1), (i + 1).min(n - 1));
    let (jl, jh) = (j.saturating_sub(1), (j + 1).min(n - 1));
    let dhdx = (grid.sample(ih, j) - grid.sample(il, j)) / ((ih - il) as f32 * step);
    let dhdz = (grid.sample(i, jh) - grid.sample(i, jl)) / ((jh - jl) as f32 * step);
    Vec3::new(-dhdx, 1.0, -dhdz).normalize().to_array()
}

/// Inclusive `[ia, ib, ja, jb]` per [`MESH_TILE_CELLS`]² tile, row-major `(tz, tx)`.
/// Ranges overlap by one node so neighbours share border positions and heights. Both
/// [`terrain_mesh_tiles`] and [`crate::terrain_lod`] use this tiling: crack-freeness needs the
/// same neighbours, not independently derived partitions.
pub(crate) fn mesh_tile_node_ranges(grid: &HeightGrid) -> Vec<[usize; 4]> {
    let cells = grid.size() as usize - 1;
    let tiles_per_side = cells.div_ceil(MESH_TILE_CELLS);
    let mut ranges = Vec::with_capacity(tiles_per_side * tiles_per_side);
    for tz in 0..tiles_per_side {
        for tx in 0..tiles_per_side {
            let ia = tx * MESH_TILE_CELLS;
            let ja = tz * MESH_TILE_CELLS;
            ranges.push([
                ia,
                (ia + MESH_TILE_CELLS).min(cells),
                ja,
                (ja + MESH_TILE_CELLS).min(cells),
            ]);
        }
    }
    ranges
}

#[cfg(test)]
// `pub(crate)` so the netcode layer's own spawn test can share this module's spawn assertion
// helper — one rule, one assertion, asserted from whichever layer owns the points.
pub(crate) mod tests {
    use super::*;

    use crate::map::tests::shipped_manifest;

    /// The shipped heightmap, decoded through the real path at the extent its manifest declares —
    /// the ground every spawn assertion is made against.
    pub(crate) fn shipped_grid() -> HeightGrid {
        let manifest = shipped_manifest();
        grid_from_png(
            &std::fs::read(manifest.heightmap_path()).expect("shipped heightmap"),
            manifest.extent,
            manifest.rows,
        )
        .expect("shipped heightmap must decode")
    }

    /// Require a spawn inside the span and above its centre and whole footprint, so uphill gear
    /// clears slopes. [`SPAWN_FOOTPRINT_HALF_M`] owns the centre-only burial counterexample.
    pub(crate) fn assert_spawn_clears_terrain(grid: &HeightGrid, name: &str, xz: Vec2) {
        assert!(
            grid.contains_xz(xz.x, xz.y),
            "{name} spawns at ({}, {}), outside the ±{} m world span",
            xz.x,
            xz.y,
            grid.half_extent(),
        );
        let spawned = spawn_pos(Some(grid), xz);
        let surface = grid.height_at(xz.x, xz.y);
        assert!(
            spawned.y >= surface + SPAWN_CLEARANCE_M,
            "{name} spawns at y {:.2} m with the surface at {surface:.2} m — {:.2} m of it is \
             underground",
            spawned.y,
            surface - spawned.y,
        );
        let footprint = grid.max_height_in_square(xz.x, xz.y, SPAWN_FOOTPRINT_HALF_M);
        assert!(
            spawned.y >= footprint + SPAWN_CLEARANCE_M,
            "{name} clears its centre point but not its footprint ({footprint:.2} m)",
        );
    }

    use avian3d::parry::math::{Pose, Vector};
    use avian3d::parry::query::Ray;

    /// The square and vertical range every synthetic fixture below is written in — the fixture
    /// extent's own, so re-hanging the fixtures needs no edit here.
    const FIXTURE_SPAN: f32 = FIXTURE_EXTENT.height_span_m;
    const FIXTURE_HALF: f32 = FIXTURE_EXTENT.half_extent();

    /// A grid whose samples come from `f(i, j)` in 8-bit terms (normalized like the decoder,
    /// no smoothing — these tests pin the raw PIECEWISE-TRIANGULAR surface, which is the
    /// collider's and deliberately not bilinear).
    fn grid_from(size: u32, f: impl Fn(u32, u32) -> u8) -> HeightGrid {
        let mut samples = Vec::with_capacity((size * size) as usize);
        for j in 0..size {
            for i in 0..size {
                samples.push(f32::from(f(i, j)) * (FIXTURE_SPAN / 255.0));
            }
        }
        HeightGrid::new(samples.into(), size, FIXTURE_EXTENT)
    }

    /// Downward parry raycast against the collider at world `(x, z)`; returns the hit height.
    fn cast_down(collider: &Collider, x: f32, z: f32) -> f32 {
        let origin_y = FIXTURE_SPAN + 100.0;
        let toi = parry_cast(collider, Vec3::new(x, origin_y, z), Vec3::NEG_Y, 1.0e5)
            .unwrap_or_else(|| panic!("terrain collider missed a vertical ray at ({x}, {z})"));
        origin_y - toi
    }

    /// Parry raycast against the collider along an arbitrary direction — the reference the
    /// exact caster is pinned against. Returns the hit `t`, or `None` on a miss.
    fn parry_cast(collider: &Collider, origin: Vec3, dir: Vec3, t_max: f32) -> Option<f32> {
        let ray = Ray::new(
            Vector::new(origin.x, origin.y, origin.z),
            Vector::new(dir.x, dir.y, dir.z),
        );
        collider
            .shape()
            .cast_ray(&Pose::IDENTITY, &ray, t_max, true)
    }

    #[test]
    fn triangular_interpolates_and_clamps_at_edges() {
        // 2x2 corners: (i=0,j=0)=0, (i=1,j=0)=255, (i=0,j=1)=0, (i=1,j=1)=255 — a pure x-ramp.
        // Planar data, so both cell triangles lie in the ramp plane: exact everywhere.
        let grid = grid_from(2, |i, _| if i == 1 { 255 } else { 0 });
        let h = FIXTURE_HALF;
        assert_eq!(grid.height_at(-h, 0.0), 0.0);
        assert!((grid.height_at(h, 0.0) - FIXTURE_SPAN).abs() < 1e-4);
        assert!((grid.height_at(0.0, 123.0) - FIXTURE_SPAN * 0.5).abs() < 1e-4);
        assert!((grid.height_at(h / 2.0, -h) - FIXTURE_SPAN * 0.75).abs() < 1e-4);
        // Placement queries clamp outside the map; the collider surface ends at the edge.
        assert_eq!(grid.height_at(-h - 500.0, 0.0), 0.0);
        assert!((grid.height_at(h + 500.0, 9999.0) - FIXTURE_SPAN).abs() < 1e-4);

        // A genuinely non-planar cell: one raised corner (h11 = H), sample the interior. On the
        // anti-diagonal split, (fu, fv) = (0.75, 0.75) lies in the upper triangle
        // {h10, h11, h01}: H + (0 − H)·0.25 + (0 − H)·0.25 = 0.5·H — NOT bilinear's 0.5625·H,
        // and NOT a main-diagonal split's 0.75·H. The collider comparison uses a separate
        // seeded grid in `oracle_matches_collider_at_seeded_points`.
        let hi = 200.0 / 255.0 * FIXTURE_SPAN;
        let corner = grid_from(2, |i, j| if i == 1 && j == 1 { 200 } else { 0 });
        assert!((corner.height_at(h / 2.0, h / 2.0) - hi * 0.5).abs() < 1e-4);
        // Dead center (fu = fv = 0.5) sits ON the split diagonal, where the raised corner
        // contributes nothing: exactly 0 (bilinear would say 0.25·H).
        assert!(corner.height_at(0.0, 0.0).abs() < 1e-4);
    }

    /// Deterministic LCG for oracle/collider probes. DERIVED: 31 high bits divided by the
    /// 32-bit maximum yield at most 0.5 after f32 rounding, restricting the callers' coverage.
    fn lcg(seed: u64) -> impl FnMut() -> f32 {
        let mut state = seed;
        move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as u32) as f32 / u32::MAX as f32 // at most 0.5; see the coverage limit above
        }
    }

    /// The seeded-noise grid every oracle-vs-collider pin below casts against.
    fn noise_grid(next: &mut impl FnMut() -> f32) -> HeightGrid {
        let size = 33u32;
        let mut samples = Vec::with_capacity((size * size) as usize);
        for _ in 0..size * size {
            samples.push(next() * FIXTURE_SPAN);
        }
        HeightGrid::new(samples.into(), size, FIXTURE_EXTENT)
    }

    /// Oracle height and exact cast must match the production parry collider within millimetres
    /// at seeded negative-X/Z points and negative-Z off-map misses. The module's
    /// one-surface invariant owns the MEASURED 0.519 m mismatch vs the 0.5 m suspension reach.
    /// Placement clamping must not extend either surface beyond the collider edge.
    #[test]
    fn oracle_matches_collider_at_seeded_points() {
        let mut next = lcg(0x243F_6A88_85A3_08D3);
        let grid = noise_grid(&mut next);
        let collider = heightfield_collider(&grid);
        let mut worst = 0.0f32;
        for k in 0..512 {
            // DERIVED: `lcg` keeps both coordinates nonpositive. The two span scales do not
            // reserve an outer band; noise avoids boundary-exact parry edge cases.
            let span = if k % 8 == 0 {
                FIXTURE_HALF * 0.9999
            } else {
                FIXTURE_HALF
            };
            let x = (next() * 2.0 - 1.0) * span;
            let z = (next() * 2.0 - 1.0) * span;
            if x.abs() >= FIXTURE_HALF || z.abs() >= FIXTURE_HALF {
                continue;
            }
            let cast = cast_down(&collider, x, z);
            let analytic = grid.height_at(x, z);
            worst = worst.max((cast - analytic).abs());
            // The exact caster reads the identical surface point.
            let origin = Vec3::new(x, FIXTURE_SPAN + 100.0, z);
            let ours = grid
                .cast_ray(origin, Vec3::NEG_Y, 1.0e5)
                .unwrap_or_else(|| panic!("cast_ray missed a vertical ray at ({x}, {z})"));
            worst = worst.max((origin.y - ours.t - analytic).abs());
        }
        // Measured: worst 6.9e-5 m over these points (height_at alone read 6.1e-5 before the
        // caster joined the pin) — the 3 mm bound is pure headroom, not observed error.
        assert!(
            worst < 3e-3,
            "oracle-vs-collider disagreement {worst} m exceeds the few-mm pin"
        );

        // Just OUTSIDE the span both surfaces must agree there is NO ground: the collider ends
        // at the edge, and the caster ends with it (a vertical ray outside misses both).
        for _ in 0..64 {
            let along = (next() * 2.0 - 1.0) * (FIXTURE_HALF + 200.0);
            let out = FIXTURE_HALF + 0.5 + next() * 200.0;
            let (x, z) = match (next() > 0.5, next() > 0.5) {
                (true, flip) => (if flip { out } else { -out }, along),
                (false, flip) => (along, if flip { out } else { -out }),
            };
            let origin = Vec3::new(x, FIXTURE_SPAN + 100.0, z);
            assert!(
                parry_cast(&collider, origin, Vec3::NEG_Y, 1.0e5).is_none(),
                "collider must end at the span edge (hit at ({x}, {z}))"
            );
            assert!(
                grid.cast_ray(origin, Vec3::NEG_Y, 1.0e5).is_none(),
                "cast_ray must end at the span edge (hit at ({x}, {z}))"
            );
        }
    }

    /// The DIRECTIONAL pin: the exact caster vs a parry cast of the production collider over a
    /// seeded sweep from negative-X/Z origins, with azimuths in the positive-Z half-turn.
    /// Both must agree on hit/no-hit at those probes, and on the hit parameter within the
    /// pinned tolerance wherever both report one. (Shallow rays amplify any surface mismatch
    /// along `t` by 1/sin(elevation), so this is the harshest agreement test the two float
    /// paths face.)
    #[test]
    fn oracle_cast_matches_collider_cast_for_seeded_directions() {
        let mut next = lcg(0x452_821E_638D_0137);
        let grid = noise_grid(&mut next);
        let collider = heightfield_collider(&grid);
        let t_max = 4_000.0_f32; // beyond the world diagonal: range never truncates a hit
        let mut worst = 0.0f32;
        let mut hits = 0u32;
        for _ in 0..512 {
            let x = (next() * 2.0 - 1.0) * FIXTURE_HALF * 0.98;
            let z = (next() * 2.0 - 1.0) * FIXTURE_HALF * 0.98;
            let origin = Vec3::new(x, grid.height_at(x, z) + 0.5 + next() * 30.0, z);
            // DERIVED from `lcg` ≤ 0.5: elevation 0.03..0.765 rad (~1.7..43.8°), azimuth
            // 0..π. Test-only trig; the production caster stays transcendental-free.
            let elevation = 0.03 + next() * 1.47;
            let azimuth = next() * core::f32::consts::TAU;
            let dir = Vec3::new(
                elevation.cos() * azimuth.cos(),
                -elevation.sin(),
                elevation.cos() * azimuth.sin(),
            );
            let theirs = parry_cast(&collider, origin, dir, t_max);
            let ours = grid.cast_ray(origin, dir, t_max).map(|hit| hit.t);
            match (ours, theirs) {
                (Some(a), Some(b)) => {
                    hits += 1;
                    worst = worst.max((a - b).abs());
                }
                (None, None) => {}
                (a, b) => panic!(
                    "hit/no-hit disagreement at origin {origin:?} dir {dir:?}: ours {a:?}, parry {b:?}"
                ),
            }
        }
        assert!(hits > 300, "the sweep must actually exercise hits ({hits})");
        // Measured: worst 3.3e-4 m of ray parameter over 509 seeded hits (graze-amplified);
        // the 5 cm pin is headroom for parry/f32 arithmetic changes, not observed error.
        assert!(
            worst < 5e-2,
            "cast_ray vs parry cast disagreement {worst} m over {hits} hits"
        );
    }

    /// Hard global bounds on the exact caster: the ±EPS_T cell-window
    /// padding is boundary-OWNERSHIP slop only. A plane root behind the origin or past `t_max`
    /// must be rejected outright — the old code accepted roots inside the padded window and
    /// clamped them onto `[0, t_max]`, turning micron-scale clearance into a full-reach
    /// penetration for an away-facing suspension probe (and admitting ballistics hits that
    /// belong to the next march chord).
    #[test]
    fn cast_ray_rejects_roots_outside_the_ray_interval() {
        let grid = grid_from(2, |_, _| 100); // flat ground, one cell spanning the world
        let ground = grid.height_at(3.0, -7.0);

        // Away-facing ray from a hair above the surface: the only crossing is BEHIND the
        // origin (t ≈ −5e-5, inside the old −EPS_T padding). Must be a miss, not a t = 0 hit.
        let origin = Vec3::new(3.0, ground + 5.0e-5, -7.0);
        assert!(
            grid.cast_ray(origin, Vec3::Y, 2.0).is_none(),
            "away-facing graze must be clearance, not clamped-to-zero contact"
        );

        // Toward-facing from just above: a genuine tiny-positive root must still hit.
        let hit = grid
            .cast_ray(Vec3::new(3.0, ground + 5.0e-3, -7.0), Vec3::NEG_Y, 2.0)
            .expect("downward graze must hit");
        assert!(
            hit.t >= 0.0 && (hit.t - 5.0e-3).abs() < 1.0e-4,
            "t = {}",
            hit.t
        );

        // Root just past `t_max` (inside the old +EPS_T padding): this chord must miss —
        // the crossing belongs to the next march segment.
        let above = Vec3::new(3.0, ground + 10.0, -7.0);
        assert!(
            grid.cast_ray(above, Vec3::NEG_Y, 10.0 - 5.0e-5).is_none(),
            "root past t_max must not be clamped onto the endpoint"
        );
        let hit = grid
            .cast_ray(above, Vec3::NEG_Y, 10.0 + 1.0e-3)
            .expect("same root inside t_max must hit");
        assert!((hit.t - 10.0).abs() < 1.0e-3, "t = {}", hit.t);

        // Origin below the surface casting further down: the crossing is far behind — a miss
        // (the oracle's explicit buried check owns that regime, not the caster).
        assert!(
            grid.cast_ray(Vec3::new(3.0, ground - 0.5, -7.0), Vec3::NEG_Y, 2.0)
                .is_none()
        );
    }

    /// Downsampling is a pure resample of the same surface: on planar data (where every
    /// resolution represents the surface exactly) the downsampled grid must reproduce the
    /// source surface at every query point.
    #[test]
    fn downsample_preserves_a_planar_surface_exactly() {
        let source = grid_from(9, |i, _| (i * 30) as u8); // planar x-ramp
        let down = downsample(&source, 5);
        assert_eq!(down.size(), 5);
        let h = FIXTURE_HALF;
        for (x, z) in [
            (-h, -h),
            (-321.5, 47.0),
            (0.0, 0.0),
            (777.25, -h / 2.0),
            (h, h),
        ] {
            let (a, b) = (source.height_at(x, z), down.height_at(x, z));
            assert!(
                (a - b).abs() < 1e-3,
                "downsample drifted at ({x}, {z}): {a} vs {b}"
            );
        }
    }

    /// Through the real manifest reader and PNG decode, `offset + normalized · span` must map
    /// full-scale zero/one to declared floor/ceiling at the declared square's corners.
    #[test]
    fn manifest_decode_maps_full_scale_onto_the_declared_range() {
        let manifest = crate::map::tests::fixture_manifest(750.0, -12.5, 50.0);
        let extent = manifest.extent;
        // 2x2 16-bit PNG: full-scale ZERO down the -X column, FULL SCALE down the +X column.
        let mut source = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::new(2, 2);
        for y in 0..2 {
            source.put_pixel(0, y, image::Luma([0]));
            source.put_pixel(1, y, image::Luma([u16::MAX]));
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageLuma16(source)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("fixture png must encode");
        let grid = grid_from_png(&png, extent, manifest.rows).expect("fixture png must decode");
        assert_eq!(grid.extent(), extent);
        let half = extent.half_extent();
        assert_eq!(half, 750.0, "the declared square is what the grid spans");
        let (floor, ceiling) = (
            extent.height_offset_m,
            extent.height_offset_m + extent.height_span_m,
        );
        assert!((grid.height_at(-half, 0.0) - floor).abs() < 1e-4);
        assert!((grid.height_at(half, 0.0) - ceiling).abs() < 1e-4);
        // Halfway across is halfway up: the mapping is affine, not merely pinned at its ends.
        assert!((grid.height_at(0.0, 0.0) - (floor + ceiling) / 2.0).abs() < 1e-4);
    }

    /// The declared row direction must place the first image row on the matching Z edge.
    /// Four distinct heights in a 2×2 fixture expose flips on either axis and transposes. Reading
    /// the shipped -Z export straight through would mirror ground under authored objects.
    #[test]
    fn the_declared_row_direction_places_the_first_image_row() {
        // Row 0 = [0, 1/3], row 1 = [2/3, 1] of full scale — every corner distinct.
        let mut source = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::new(2, 2);
        let third = u16::MAX / 3;
        source.put_pixel(0, 0, image::Luma([0]));
        source.put_pixel(1, 0, image::Luma([third]));
        source.put_pixel(0, 1, image::Luma([third * 2]));
        source.put_pixel(1, 1, image::Luma([u16::MAX]));
        let mut png = Vec::new();
        image::DynamicImage::ImageLuma16(source)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("fixture png must encode");

        let corner = |rows: &str, x: f32, z: f32| {
            let manifest = crate::map::tests::fixture_manifest_with_rows(500.0, 0.0, 100.0, rows);
            let grid =
                grid_from_png(&png, manifest.extent, manifest.rows).expect("fixture png decodes");
            grid.height_at(x, z)
        };
        let (half, span) = (500.0_f32, 100.0_f32);
        let height = |raw: u16| span * f32::from(raw) / f32::from(u16::MAX);
        let row_0 = [height(0), height(third)];
        let row_1 = [height(third * 2), height(u16::MAX)];
        // "+Z": the first image row is the -Z edge, straight through.
        assert!((corner("+Z", -half, -half) - row_0[0]).abs() < 1e-3);
        assert!((corner("+Z", half, -half) - row_0[1]).abs() < 1e-3);
        assert!((corner("+Z", -half, half) - row_1[0]).abs() < 1e-3);
        // "-Z": the first image row is the +Z edge — the slab is reversed row-wise, and ONLY
        // row-wise (the columns still run toward +X).
        assert!((corner("-Z", -half, half) - row_0[0]).abs() < 1e-3);
        assert!((corner("-Z", half, half) - row_0[1]).abs() < 1e-3);
        assert!((corner("-Z", -half, -half) - row_1[0]).abs() < 1e-3);
        assert!((corner("-Z", half, -half) - row_1[1]).abs() < 1e-3);
    }

    /// Multi-tile chunking is culling-only: tiles cover every cell exactly once, border vertices
    /// are shared (same world position, same height), and every vertex still sits on the ONE
    /// surface — the vertices are the grid samples themselves, and every cell splits along the
    /// same anti-diagonal parry uses (the index pattern's shared edge is `i01`–`i10`).
    #[test]
    fn mesh_tiling_is_seamless_and_stays_on_the_surface() {
        // 131 nodes = 130 cells → 2×2 tiles (128 + 2 cells per side).
        let size = 131u32;
        let mut samples = Vec::with_capacity((size * size) as usize);
        for j in 0..size {
            for i in 0..size {
                samples.push(((i * 7 + j * 13) % 97) as f32 * 0.5);
            }
        }
        let grid = HeightGrid::new(samples.into(), size, FIXTURE_EXTENT);
        let tiles = terrain_mesh_tiles(&grid);
        assert_eq!(tiles.len(), 4);
        let mut triangles = 0;
        for (mesh, [ia, ib, _, _]) in tiles.iter().zip(mesh_tile_node_ranges(&grid)) {
            let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("terrain mesh must carry f32x3 positions");
            };
            for p in positions {
                // Vertices must BE grid samples (not merely near the interpolated surface):
                // recover the node indices from the exact world position and compare raw.
                let step = FIXTURE_EXTENT.world_size_m / (size - 1) as f32;
                let i = ((p[0] + FIXTURE_HALF) / step).round() as usize;
                let j = ((p[2] + FIXTURE_HALF) / step).round() as usize;
                let expected = grid.sample(i, j);
                assert_eq!(
                    p[1], expected,
                    "tile vertex {p:?} is not the grid sample at node ({i}, {j})"
                );
            }
            let Some(Indices::U32(indices)) = mesh.indices() else {
                panic!("terrain mesh must carry u32 indices");
            };
            // The first cell's six indices: triangles {i00, i01, i10} and {i10, i01, i11} share
            // the i01–i10 edge — the anti-diagonal ((x_lo, z_hi)–(x_hi, z_lo)), same as
            // parry/height_at.
            let w = (ib - ia + 1) as u32;
            assert_eq!(&indices[..6], &[0, w, 1, 1, w, w + 1][..]);
            triangles += indices.len() / 3;
        }
        assert_eq!(
            triangles,
            130 * 130 * 2,
            "tiles must cover every cell exactly once"
        );
    }

    /// CI must catch missing maps, LFS pointers, PNG/JPG single-mip regressions (MEASURED 30 fps),
    /// and missing `basis-universal` support; `world::report_failed_terrain_map` runs only with a
    /// window. All three bound maps need mipmapped KTX2 headers; albedo represents their shared
    /// transcode law through Bevy's loader.
    /// Transcode target follows flags, not the host GPU: `CompressedImageFormats::BC` chooses BC7,
    /// Apple Silicon uses ASTC 4x4 (`get_transcoded_formats`), both 8 bpp. Albedo requests sRGB as
    /// `world::MapEncoding` does.
    #[test]
    fn shipped_terrain_surface_maps_are_mipmapped_ktx2() {
        use bevy::image::{CompressedImageFormats, ktx2_buffer_to_image};
        // «KTX 20»\r\n\x1a\n — the KTX2 file identifier.
        const KTX2_MAGIC: &[u8] = b"\xabKTX 20\xbb\r\n\x1a\n";
        // Header fields after the 12-byte identifier, each a little-endian u32.
        const PIXEL_WIDTH_AT: usize = 20;
        const PIXEL_HEIGHT_AT: usize = 24;
        const LEVEL_COUNT_AT: usize = 40;
        let header_u32 = |bytes: &[u8], at: usize| {
            u32::from_le_bytes(bytes[at..at + 4].try_into().expect("a 4-byte header field"))
        };
        let read = |path: &str, what: &str| {
            let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join(path);
            let bytes = std::fs::read(&full)
                .unwrap_or_else(|err| panic!("terrain map missing at {}: {err}", full.display()));
            assert!(
                bytes.len() > 4096,
                "{path} is {} bytes — a Git LFS POINTER, not the map",
                bytes.len()
            );
            assert!(
                bytes.starts_with(KTX2_MAGIC),
                "the {what} map {path} must be KTX2 — rebuild it with scripts/encode-terrain-ktx2.sh"
            );
            bytes
        };
        for (path, what) in [
            (TEXTURE_PATH, "albedo"),
            (NORMAL_PATH, "normal"),
            (ARM_PATH, "arm"),
        ] {
            let bytes = read(path, what);
            let width = header_u32(&bytes, PIXEL_WIDTH_AT);
            let height = header_u32(&bytes, PIXEL_HEIGHT_AT);
            let levels = header_u32(&bytes, LEVEL_COUNT_AT);
            // A full chain runs down to 1×1. A map that lost it reports 1 and thrashes the texture
            // cache at every grazing angle.
            let full_chain = width
                .max(height)
                .checked_ilog2()
                .unwrap_or_else(|| panic!("the {what} map declares a {width}×{height} image"))
                + 1;
            assert_eq!(
                levels, full_chain,
                "the {what} map carries {levels} mip levels, not the full chain of its \
                 {width}×{height}"
            );
        }
        let image = ktx2_buffer_to_image(
            &read(TEXTURE_PATH, "albedo"),
            CompressedImageFormats::BC,
            true,
        )
        .unwrap_or_else(|err| panic!("{TEXTURE_PATH} failed to transcode: {err:?}"));
        let format = format!("{:?}", image.texture_descriptor.format);
        assert_eq!(
            format, "Bc7RgbaUnormSrgb",
            "the albedo map transcoded to {format}"
        );
    }

    /// `world` requires mikktspace tangents on every tile (ADR-0011): triangle-list positions,
    /// normals, UV0 and indices must survive tiler changes. Without a tangent basis, normal-map
    /// directions have no frame and ground renders flat/greasy.
    #[test]
    fn render_mesh_tiles_can_carry_a_tangent_basis() {
        let grid = grid_from(9, |i, j| (i * 20 + j * 10) as u8);
        for mut mesh in terrain_mesh_tiles(&grid) {
            mesh.generate_tangents()
                .expect("terrain tiles must support mikktspace tangent generation");
            let Some(bevy::mesh::VertexAttributeValues::Float32x4(tangents)) =
                mesh.attribute(Mesh::ATTRIBUTE_TANGENT)
            else {
                panic!("tangent generation must leave an f32x4 tangent attribute");
            };
            assert_eq!(tangents.len(), 9 * 9, "one tangent per grid node");
            for t in tangents {
                let axis = Vec3::new(t[0], t[1], t[2]);
                assert!(
                    (axis.length() - 1.0).abs() < 1e-3,
                    "tangent {t:?} is not unit-length"
                );
                assert!(
                    (t[3].abs() - 1.0).abs() < 1e-3,
                    "tangent {t:?} must carry a ±1 handedness sign"
                );
            }
        }
    }

    /// Sim-owned spawns must clear real terrain at their XZ and footprint. See
    /// [`spawn_surface_height`] for the MEASURED ~116 m fixed-Y counterexample.
    /// Net-owned lane, bot and map-clamp spawns use the same [`assert_spawn_clears_terrain`] in
    /// `net::server`: `src/net/tests/net_boundary.rs` forbids sim dependencies on `crate::net`,
    /// keeping single-player runnable without it. Cover the generated far probe layout too: the first
    /// 28 indices represent a 30-tank block; `tank::scenario::probe_spawn_xz` owns their positions.
    #[test]
    fn every_sim_spawn_point_lands_above_the_shipped_terrain() {
        use crate::tank::scenario::{duel_spawn_xz, probe_spawn_xz};

        let grid = shipped_grid();
        // The count is capped by `OVERMATCH_PROBE_TANKS`, so a probe INDEX has no upper bound in
        // principle. 28 is the block a 30-tank sweep spawns and the only one anyone places by hand.
        const PROBES: usize = 28;
        for far in [false, true] {
            let placement = if far { "far" } else { "near" };
            for (i, xz) in duel_spawn_xz(far).iter().enumerate() {
                assert_spawn_clears_terrain(&grid, &format!("{placement} offline duel {i}"), *xz);
            }
            for i in 0..PROBES {
                assert_spawn_clears_terrain(
                    &grid,
                    &format!("{placement} probe tank {i}"),
                    probe_spawn_xz(far, i),
                );
            }
        }
    }

    /// A spawn point off the map is a code bug, and the rule says so instead of handing back an
    /// edge-clamped height for a point in the void (where the collider does not exist, so the tank
    /// would fall forever).
    #[test]
    #[should_panic(expected = "outside the")]
    fn a_spawn_outside_the_world_span_fails_loud() {
        let grid = grid_from(9, |_, _| 40);
        spawn_pos(Some(&grid), Vec2::new(FIXTURE_HALF + 1.0, 0.0));
    }

    /// A frictionless box, pitched and rotation-locked with its leading bottom edge lowest, must
    /// retain horizontal +Z speed across flat grid lines. Without `FIX_INTERNAL_EDGES`, a
    /// triangle-edge normal tilts horizontally and stops it within a tick, as with a hull's rear
    /// edge reversing off a slope. Every terrain-supported dynamic body relies on this.
    /// Box, pitch, speed and DERIVED 1.465 m shipped-map spacing are FIXTURE data.
    #[test]
    fn a_frictionless_edge_slides_across_flat_terrain_at_speed() {
        use avian3d::prelude::{
            LinearVelocity, LockedAxes, PhysicsPlugins, Position, RigidBody, Rotation,
        };
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        const SIZE: u32 = 33;
        const SPACING_M: f32 = 1.465;
        const SPEED: f32 = 4.0;
        const PITCH_DEG: f32 = 10.0;
        let extent = TerrainExtent {
            world_size_m: (SIZE - 1) as f32 * SPACING_M,
            ..FIXTURE_EXTENT
        };
        let grid = HeightGrid::new(vec![0.0; (SIZE * SIZE) as usize].into(), SIZE, extent);

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app.world_mut()
            .spawn((RigidBody::Static, heightfield_collider(&grid)));
        // Pitched about +X, the box's +Z bottom edge is its lowest point: that edge leads.
        let half = Vec3::new(1.0, 0.25, 1.5);
        let pitch = Quat::from_rotation_x(PITCH_DEG.to_radians());
        let lowest = (pitch * Vec3::new(0.0, -half.y, half.z)).y;
        let body = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Collider::cuboid(2.0 * half.x, 2.0 * half.y, 2.0 * half.z),
                crate::substances::contact_friction(0.0),
                LockedAxes::ROTATION_LOCKED,
                Position(Vec3::new(0.3, 0.005 - lowest, -8.0 * SPACING_M - 0.4)),
                Rotation(pitch),
                LinearVelocity(Vec3::Z * SPEED),
            ))
            .id();
        let mut slowest = SPEED;
        let mut grid_lines = 0.0;
        let start = app.world().get::<Position>(body).unwrap().0.z;
        for _ in 0..128 {
            app.update();
            let v = app.world().get::<LinearVelocity>(body).unwrap().0;
            slowest = slowest.min(v.z);
            grid_lines = (app.world().get::<Position>(body).unwrap().0.z - start) / SPACING_M;
        }
        assert!(
            slowest > 0.95 * SPEED,
            "a frictionless box on flat terrain fell from {SPEED} m/s to {slowest:.2} m/s after \
             {grid_lines:.1} grid lines: a terrain contact pushed it backwards (an internal-edge \
             normal)",
        );
        assert!(
            grid_lines > 4.0,
            "the box crossed only {grid_lines:.1} grid lines: the fixture did not reach the edges",
        );
    }
}
