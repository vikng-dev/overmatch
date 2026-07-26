//! The world height grid: `assets/terrain/terrain_height.png` decoded synchronously at startup
//! (ADR-0014 — sim construction never waits on the async asset server) into a shared, immutable
//! sample slab that every terrain representation derives from:
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
//! There is exactly ONE ground surface, and every consumer reads the SAME one:
//!
//! 1. The decoded map is downsampled ONCE to [`GRID_RESOLUTION`]² (2.5 m spacing) and THAT grid
//!    is the [`HeightGrid`] resource. No consumer ever sees the full-resolution decode.
//! 2. [`HeightGrid::height_at`] is piecewise-TRIANGULAR, splitting every cell along the SAME
//!    diagonal parry's heightfield triangulation uses (the anti-diagonal — pinned empirically by
//!    `parry_splits_cells_along_the_anti_diagonal` and `oracle_matches_collider_at_seeded_points`
//!    below). So oracle == collider at every point, up to float rounding.
//! 3. The collider ([`heightfield_collider`]) and the render mesh ([`terrain_mesh_tiles`]) are
//!    built from the grid's OWN samples — never resampled at a different resolution — with that
//!    same diagonal split. Oracle, collider, and visuals are the identical surface.
//!
//! History: before this invariant the oracle read a 4096² bilinear surface, the collider a 1025²
//! triangulated resample, and the render mesh a 513² resample — measured to disagree by up to
//! 0.519 m (oracle vs collider, more than the suspension's 0.5 m probe reach) and 1.46 m
//! (oracle vs render): the hull could touch ground the belts never felt. Do not reintroduce a
//! second resolution anywhere.
//!
//! One decode, one mapping, identical bytes on every peer (the PNG ships in `assets/` on the
//! client archives AND the server tar — see `.github/actions/build-server`), so the
//! deterministic sim reads the same ground everywhere. When the PNG is absent the resource is
//! simply not inserted and `world` falls back to the flat slab + authored test course.

use std::sync::Arc;

use avian3d::prelude::Collider;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;

/// Side length (m) of the square heightmap world. The map was AUTHORED at a 2.5 km side: 4096 px
/// spanning 2560 m is 0.625 m/px. Single home for the world mapping — the oracle term, collider,
/// render mesh, and spawn queries all derive from this constant pair.
pub const WORLD_SIZE: f32 = 2560.0;

/// Half the world side (m): world XZ spans `[-WORLD_HALF_EXTENT, +WORLD_HALF_EXTENT]`, centered
/// on the origin. Exposed for spawn-map bounds clamping / UV mapping (`net::spawn_map`).
pub const WORLD_HALF_EXTENT: f32 = WORLD_SIZE / 2.0;

/// Vertical range (m): a full-scale sample maps to this, zero to 0 m — CONFIRMED by the map's
/// author (0..255 → 0..150 m). At 8 bits that quantizes height in ~0.588 m steps (150/255); a
/// 16-bit re-export is expected from the author later, and the decode path already normalizes
/// samples to full-scale-relative values regardless of bit depth, so a 16-bit grayscale PNG is a
/// drop-in replacement (no code change).
pub const HEIGHT_RANGE: f32 = 150.0;

/// The heightmap, relative to the resolved asset root (`crate::assets::asset_root`).
const HEIGHT_MAP_PATH: &str = "terrain/terrain_height.png";

/// Import-time Gaussian smoothing width, in source pixels; `0.0` = pass-through (the current
/// setting — the shipped map is the author's 16-bit export, whose ~2.3 mm quantization steps
/// need no de-terracing).
///
/// Why it exists: an 8-BIT source quantizes height in 150/255 ≈ 0.588 m steps (±0.294 m error)
/// — terracing at real obstacle scale for the suspension. The separable Gaussian
/// ([`gaussian_smooth`]) removes that BOUNDED quantization noise (≤ half a step) at the cost of
/// rounding real terrain features smaller than ~the kernel radius (3σ ≈ 11 m on the ground at
/// 0.625 m/px). Set this back to [`SMOOTH_KERNEL_SIGMA`] if an 8-bit map ever ships again.
///
/// Determinism: the blur is pure f32 arithmetic in a fixed order over identical bytes, so every
/// peer computes the same grid bit-for-bit. The kernel weights are EMBEDDED CONSTANTS
/// ([`SMOOTH_KERNEL`]) rather than `exp()` calls — libm's `exp` is not bit-identical across
/// platforms, and the grid feeds the deterministic belt sim. No threading, no reduction
/// reordering.
pub(crate) const SMOOTH_SIGMA_PX: f32 = 0.0;

/// The σ (px) the embedded [`SMOOTH_KERNEL`] table was generated for. When smoothing is active,
/// `SMOOTH_SIGMA_PX` must equal this (or the table must be regenerated); the
/// `smooth_kernel_matches_its_sigma` test pins table ↔ σ.
pub(crate) const SMOOTH_KERNEL_SIGMA: f32 = 6.0;

/// Normalized Gaussian half-kernel for [`SMOOTH_KERNEL_SIGMA`] = 6.0, radius 3σ = 18,
/// edge-clamped at use. `SMOOTH_KERNEL[k]` is the weight at offset ±k; `w[0] + 2·Σ w[1..] = 1`.
const SMOOTH_KERNEL: [f32; 19] = [
    0.066_625_13,     // k = 0
    0.065_706_18,     // k = 1
    0.063_024_68,     // k = 2
    0.058_796_473,    // k = 3
    0.053_349_235,    // k = 4
    0.047_080_535,    // k = 5
    0.040_410_185,    // k = 6
    0.033_734_677,    // k = 7
    0.027_390_411,    // k = 8
    0.021_630_013,    // k = 9
    0.016_613_124,    // k = 10
    0.012_410_294,    // k = 11
    0.009_016_731,    // k = 12
    0.006_371_657_5,  // k = 13
    0.004_379_172,    // k = 14
    0.002_927_304,    // k = 15
    0.001_903_180_3,  // k = 16
    0.001_203_450_7,  // k = 17
    0.000_740_138_36, // k = 18
];

/// Sample count per side of THE ground surface (2.5 m spacing over the 2560 m world): the decode
/// is downsampled ONCE to this, and the resulting grid is what the oracle, the collider, the
/// render mesh, and spawn placement all read — see the ONE-SURFACE INVARIANT in the module doc.
/// (The full 4096² decode is too heavy for parry, so the shared resolution is the collider's.)
pub(crate) const GRID_RESOLUTION: u32 = 1025;

/// Render-mesh tile size in CELLS per side: the mesh carries the grid's own 1025² vertices (the
/// one surface, never resampled), chunked into 8×8 tiles of 128² cells so bevy's per-entity
/// frustum culling works instead of drawing ~2.1M triangles every frame.
pub(crate) const MESH_TILE_CELLS: usize = 128;

/// World meters per tile of the terrain diffuse: the mesh's UVs are `world_xz / this`, sampled
/// with a REPEAT-addressing sampler, so the placeholder 4k texture repeats every 8 m — ground
/// detail at tank scale without a projector-obvious macro pattern. View-only (the UVs live on
/// the client render mesh; grid/oracle/collider are untouched by texturing).
pub(crate) const TEXTURE_TILE_M: f32 = 8.0;

/// The placeholder terrain diffuse (Poly Haven `coast_sand_rocks_02`, CC0 — see
/// `assets/terrain/cc.txt`), relative to the asset root.
pub(crate) const TEXTURE_PATH: &str = "terrain/coast_sand_rocks_02_diff_4k.jpg";

/// Opt-out marker: insert BEFORE the first update to keep the flat slab + authored test course
/// even when the heightmap PNG is present. Test fixtures (`headless_test`, whose transmission
/// gates drive the authored ramps) and the armor sandbox insert this; product compositions do not.
#[derive(Resource, Default)]
pub struct ForceFlatWorld;

/// The decoded height grid: heights in METERS as f32 (row-major, row = z, column = x), already
/// bit-depth-normalized (8-bit `v` → `v / 255`, 16-bit → `v / 65535`, then × [`HEIGHT_RANGE`])
/// and import-smoothed ([`SMOOTH_SIGMA_PX`]) — every consumer reads THIS one truth, so the sim,
/// collider, and visuals cannot diverge. Shared via `Arc` so the oracle's copy inside
/// [`crate::track::terrain::TrackField`] is free.
///
/// Mapping (the single convention every consumer uses): sample `(i, j)` sits at
/// `x = -WORLD_HALF_EXTENT + i * WORLD_SIZE / (size - 1)` (same for `z` with `j`), so the grid
/// spans `[-WORLD_HALF_EXTENT, +WORLD_HALF_EXTENT]` inclusive on both axes. Outside the span
/// the map clamps (the edge continues flat).
#[derive(Resource, Clone)]
pub struct HeightGrid {
    samples: Arc<[f32]>,
    /// Samples per side (4096 for the shipped map; tests build smaller grids).
    size: u32,
}

impl HeightGrid {
    /// Build from height samples in meters; `samples.len()` must be `size * size` and
    /// `size >= 2`.
    pub fn new(samples: Arc<[f32]>, size: u32) -> Self {
        assert!(size >= 2, "height grid needs at least 2 samples per side");
        assert_eq!(
            samples.len(),
            (size as usize) * (size as usize),
            "height grid sample count must be size²"
        );
        Self { samples, size }
    }

    /// Samples per side.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// World half-extent (m) this grid spans — `WORLD_HALF_EXTENT`, exposed as an accessor so
    /// consumers holding a grid need not name the constant.
    pub fn half_extent(&self) -> f32 {
        WORLD_HALF_EXTENT
    }

    /// One sample (meters).
    fn sample(&self, i: usize, j: usize) -> f32 {
        self.samples[j * self.size as usize + i]
    }

    /// Terrain height (m) at world `(x, z)` — piecewise TRIANGULAR over the grid, clamped at the
    /// map edges (the surface continues flat outward). Pure arithmetic, no platform-varying ops:
    /// this is the ground surface the DETERMINISTIC belt sim probes.
    ///
    /// THE ONE-SURFACE INVARIANT (module doc): every cell is split along the SAME diagonal
    /// parry's heightfield triangulation uses — the ANTI-diagonal, connecting the cell's
    /// `(x_lo, z_hi)` and `(x_hi, z_lo)` corners (parry `triangles_at`, default non-zigzag
    /// status: triangles `(p00, p10, p01)` / `(p10, p11, p01)`; pinned empirically by the
    /// `parry_splits_cells_along_the_anti_diagonal` and `oracle_matches_collider_at_seeded_points`
    /// tests). So this function IS the collider surface, up to float rounding — not an
    /// approximation of it. Do NOT change this back to bilinear without changing the collider.
    ///
    /// Intended consumers beyond the oracle: the net server's spawn/respawn placement
    /// (`src/net/server.rs` reads `Res<HeightGrid>` — see also [`Self::max_height_in_square`]).
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let n = self.size as usize;
        let last = (n - 1) as f32;
        let u = ((x + WORLD_HALF_EXTENT) / WORLD_SIZE * last).clamp(0.0, last);
        let v = ((z + WORLD_HALF_EXTENT) / WORLD_SIZE * last).clamp(0.0, last);
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

    /// The MAXIMUM grid height (m) over an axis-aligned square footprint of half-side `half`
    /// centered at world `(x, z)` — the spawn-placement query (a tank dropped at
    /// `height_at(center)` on a slope spawns with its uphill running gear buried; measured
    /// 1.65 m of axle burial on a legal spawn click before this existed).
    ///
    /// Conservative by construction: it takes the max over every grid NODE of every cell the
    /// square touches (the enclosing node rectangle). The surface is piecewise linear with its
    /// extremes at nodes, so this bounds the true surface maximum over the square from above.
    /// Pure and deterministic like [`Self::height_at`].
    pub fn max_height_in_square(&self, x: f32, z: f32, half: f32) -> f32 {
        let n = self.size as usize;
        let last = (n - 1) as f32;
        let node = |w: f32| ((w + WORLD_HALF_EXTENT) / WORLD_SIZE * last).clamp(0.0, last);
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

    /// Deterministic content fingerprint for the bitprobe startup dump (exact bit patterns, so
    /// any cross-platform grid divergence shows up here first).
    #[cfg(feature = "bitprobe")]
    pub(crate) fn byte_sum(&self) -> u32 {
        self.samples
            .iter()
            .fold(0u32, |acc, &s| acc.wrapping_add(s.to_bits()))
    }
}

/// Decode the heightmap synchronously at Startup (before `world::spawn_environment`, which is
/// chained after this in `world::plugin`). Missing file → no resource → the flat slab + test
/// course world (deleting the PNG restores the old world). A PRESENT but undecodable/non-square
/// map is a broken ship and panics (ADR-0011 fail-fast) — a peer silently falling back to flat
/// while others load the map would desync the deterministic sim.
pub(crate) fn decode_height_grid(mut commands: Commands, flat: Option<Res<ForceFlatWorld>>) {
    if flat.is_some() {
        info!("terrain: ForceFlatWorld set — keeping the flat slab + authored course");
        return;
    }
    let path = crate::assets::asset_root().join(HEIGHT_MAP_PATH);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => {
            info!(
                "terrain: no heightmap at {} ({err}) — flat world",
                path.display()
            );
            return;
        }
    };
    let grid = grid_from_png(&bytes)
        .unwrap_or_else(|err| panic!("terrain: {} failed to decode: {err}", path.display()));
    info!(
        "terrain: height grid {size}x{size} decoded ({world} m span, 0..{range} m, smoothing σ = {sigma} px)",
        size = grid.size(),
        world = WORLD_SIZE,
        range = HEIGHT_RANGE,
        sigma = SMOOTH_SIGMA_PX,
    );
    commands.insert_resource(grid);
}

/// PNG bytes → [`HeightGrid`]: decode, bit-depth-normalize, and (when [`SMOOTH_SIGMA_PX`] is
/// active) smooth. Pure, so the shipped asset itself is pinned by a unit test — which also
/// makes a Git-LFS pointer file shipping in place of the map fail in CI instead of at boot.
pub(crate) fn grid_from_png(bytes: &[u8]) -> Result<HeightGrid, String> {
    let image = image::load_from_memory(bytes).map_err(|err| err.to_string())?;
    // Normalize to 0..1 of full scale regardless of source bit depth, then scale to meters —
    // the author's 16-bit export and the earlier 8-bit map interchange freely.
    let (samples, width, height): (Vec<f32>, u32, u32) = match image {
        image::DynamicImage::ImageLuma8(buf) => {
            let (w, h) = buf.dimensions();
            let samples = buf
                .into_raw()
                .into_iter()
                .map(|v| f32::from(v) * (HEIGHT_RANGE / 255.0))
                .collect();
            (samples, w, h)
        }
        image::DynamicImage::ImageLuma16(buf) => {
            let (w, h) = buf.dimensions();
            let samples = buf
                .into_raw()
                .into_iter()
                .map(|v| f32::from(v) * (HEIGHT_RANGE / 65535.0))
                .collect();
            (samples, w, h)
        }
        other => {
            let buf = other.into_luma16();
            let (w, h) = buf.dimensions();
            let samples = buf
                .into_raw()
                .into_iter()
                .map(|v| f32::from(v) * (HEIGHT_RANGE / 65535.0))
                .collect();
            (samples, w, h)
        }
    };
    if width != height {
        return Err(format!("must be square, got {width}x{height}"));
    }
    // The import-time de-terracing pass (see `SMOOTH_SIGMA_PX`; currently 0.0 = pass-through
    // for the 16-bit source): when active, smoothed BEFORE the resource exists, so the oracle,
    // collider, mesh, and spawn queries all read one smoothed truth.
    let samples = if SMOOTH_SIGMA_PX > 0.0 {
        assert_eq!(
            SMOOTH_SIGMA_PX, SMOOTH_KERNEL_SIGMA,
            "terrain: SMOOTH_SIGMA_PX changed without regenerating SMOOTH_KERNEL"
        );
        gaussian_smooth(&samples, width as usize)
    } else {
        samples
    };
    let grid = HeightGrid::new(samples.into(), width);
    // THE ONE-SURFACE INVARIANT (module doc): the decode is resampled ONCE, here, down to
    // GRID_RESOLUTION — the resource every consumer reads. A map already at or below that
    // resolution passes through untouched (tests build tiny grids directly).
    if grid.size() > GRID_RESOLUTION {
        Ok(downsample(&grid, GRID_RESOLUTION))
    } else {
        Ok(grid)
    }
}

/// Resample `grid` onto a `target`² node lattice over the same world square: each new node reads
/// `height_at` at its own world position (the source's triangular surface — pure f32 arithmetic
/// in a fixed order, so every peer downsamples bit-identically). Runs ONCE at decode; after it,
/// the source resolution no longer exists anywhere.
pub(crate) fn downsample(grid: &HeightGrid, target: u32) -> HeightGrid {
    let n = target as usize;
    let step = WORLD_SIZE / (n - 1) as f32;
    let mut samples = Vec::with_capacity(n * n);
    for j in 0..n {
        let z = -WORLD_HALF_EXTENT + j as f32 * step;
        for i in 0..n {
            let x = -WORLD_HALF_EXTENT + i as f32 * step;
            samples.push(grid.height_at(x, z));
        }
    }
    HeightGrid::new(samples.into(), target)
}

/// Separable, edge-clamped Gaussian blur over a square grid — the [`SMOOTH_SIGMA_PX`]
/// de-terracing pass. Pure f32 arithmetic in a fixed accumulation order per output (center tap,
/// then symmetric pairs outward), sequential, so identical input bytes give bit-identical
/// output on every peer. The vertical pass runs as transpose → row pass → transpose:
/// transposition moves values without arithmetic and every output keeps the exact same tap
/// order, so the result is bit-identical to the naive strided pass (verified: identical
/// checksum) at ~3× the speed — ~0.45 s for the 4096² grid at `-O` (measured, Apple Silicon).
pub(crate) fn gaussian_smooth(samples: &[f32], size: usize) -> Vec<f32> {
    let pass_rows = |src: &[f32]| -> Vec<f32> {
        let mut out = vec![0.0f32; src.len()];
        for j in 0..size {
            let row = &src[j * size..(j + 1) * size];
            let orow = &mut out[j * size..(j + 1) * size];
            for i in 0..size {
                let mut acc = row[i] * SMOOTH_KERNEL[0];
                for (k, w) in SMOOTH_KERNEL.iter().enumerate().skip(1) {
                    let lo = i.saturating_sub(k);
                    let hi = (i + k).min(size - 1);
                    acc += (row[lo] + row[hi]) * w;
                }
                orow[i] = acc;
            }
        }
        out
    };
    let transpose = |src: &[f32]| -> Vec<f32> {
        let mut out = vec![0.0f32; src.len()];
        for j in 0..size {
            for i in 0..size {
                out[i * size + j] = src[j * size + i];
            }
        }
        out
    };
    // Horizontal (along x), then vertical (along z) via the transpose sandwich.
    let horizontal = pass_rows(samples);
    transpose(&pass_rows(&transpose(&horizontal)))
}

/// The one static terrain collider: an Avian/parry heightfield whose nodes ARE the grid's own
/// samples (the ONE-SURFACE invariant — never resampled at another resolution), spanning the
/// full `[-WORLD_HALF_EXTENT, +WORLD_HALF_EXTENT]` square.
///
/// Orientation is pinned EMPIRICALLY by `heightfield_matches_grid_orientation_and_scale` below
/// (the classic transposed-heightfield bug, caught by that test's first run): through avian
/// 0.7's `Collider::heightfield(Vec<Vec<f32>>, scale)`, the OUTER Vec index runs along world X
/// and the INNER index along world Z (avian flattens row-major into a parry matrix whose
/// effective layout lands this way — trust the raycast, not the doc comments). The field is
/// centered on the origin with `scale.x`/`scale.z` as TOTAL spans and `scale.y` multiplying the
/// raw heights. Parry splits each cell along the anti-diagonal, which is exactly what
/// [`HeightGrid::height_at`] evaluates — `oracle_matches_collider_at_seeded_points` pins the
/// two surfaces together to millimetres.
pub(crate) fn heightfield_collider(grid: &HeightGrid) -> Collider {
    let n = grid.size() as usize;
    let mut rows: Vec<Vec<f32>> = Vec::with_capacity(n);
    for i in 0..n {
        let mut row = Vec::with_capacity(n);
        for j in 0..n {
            row.push(grid.sample(i, j));
        }
        rows.push(row);
    }
    Collider::heightfield(rows, Vec3::new(WORLD_SIZE, 1.0, WORLD_SIZE))
}

/// The client-only terrain render meshes: the grid's OWN samples as vertices (the ONE-SURFACE
/// invariant — the drawn ground is the identical surface the collider and oracle read, cell
/// diagonals included), chunked into [`MESH_TILE_CELLS`]² tiles purely so bevy can frustum-cull
/// per entity. Positions are world-space (tiles spawn at `Transform::IDENTITY`); adjacent tiles
/// share their border row of vertices, so the surface is seamless. Normals are central
/// differences of the grid (view-only shading), UVs world-space-tiled
/// (`world_xz / TEXTURE_TILE_M` — repeat-sampled, see [`TEXTURE_TILE_M`]). The dedicated server
/// never builds this (`world::spawn_environment` gates it on a window existing).
pub(crate) fn terrain_mesh_tiles(grid: &HeightGrid) -> Vec<Mesh> {
    let n = grid.size() as usize;
    let cells = n - 1;
    let step = WORLD_SIZE / cells as f32;
    let tiles_per_side = cells.div_ceil(MESH_TILE_CELLS);
    let world_at = |k: usize| -WORLD_HALF_EXTENT + k as f32 * step;
    // Shading normal at node (i, j): central differences of the grid, one-sided at the edges.
    let normal_at = |i: usize, j: usize| -> [f32; 3] {
        let (il, ih) = (i.saturating_sub(1), (i + 1).min(n - 1));
        let (jl, jh) = (j.saturating_sub(1), (j + 1).min(n - 1));
        let dhdx = (grid.sample(ih, j) - grid.sample(il, j)) / ((ih - il) as f32 * step);
        let dhdz = (grid.sample(i, jh) - grid.sample(i, jl)) / ((jh - jl) as f32 * step);
        Vec3::new(-dhdx, 1.0, -dhdz).normalize().to_array()
    };
    let mut meshes = Vec::with_capacity(tiles_per_side * tiles_per_side);
    for tz in 0..tiles_per_side {
        for tx in 0..tiles_per_side {
            // Node range of this tile, inclusive: the last row/column is shared with the next
            // tile (same world positions and heights, so no seam).
            let ia = tx * MESH_TILE_CELLS;
            let ib = (ia + MESH_TILE_CELLS).min(cells);
            let ja = tz * MESH_TILE_CELLS;
            let jb = (ja + MESH_TILE_CELLS).min(cells);
            let (w, h) = (ib - ia + 1, jb - ja + 1);
            let mut positions: Vec<[f32; 3]> = Vec::with_capacity(w * h);
            let mut normals: Vec<[f32; 3]> = Vec::with_capacity(w * h);
            let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(w * h);
            for j in ja..=jb {
                let z = world_at(j);
                for i in ia..=ib {
                    let x = world_at(i);
                    positions.push([x, grid.sample(i, j), z]);
                    normals.push(normal_at(i, j));
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
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_indices(Indices::U32(indices));
            meshes.push(mesh);
        }
    }
    meshes
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::parry::math::{Pose, Vector};
    use avian3d::parry::query::Ray;

    /// A grid whose samples come from `f(i, j)` in 8-bit terms (normalized like the decoder,
    /// no smoothing — these tests pin the raw bilinear surface).
    fn grid_from(size: u32, f: impl Fn(u32, u32) -> u8) -> HeightGrid {
        let mut samples = Vec::with_capacity((size * size) as usize);
        for j in 0..size {
            for i in 0..size {
                samples.push(f32::from(f(i, j)) * (HEIGHT_RANGE / 255.0));
            }
        }
        HeightGrid::new(samples.into(), size)
    }

    /// Downward parry raycast against the collider at world `(x, z)`; returns the hit height.
    fn cast_down(collider: &Collider, x: f32, z: f32) -> f32 {
        let origin_y = HEIGHT_RANGE + 100.0;
        let ray = Ray::new(Vector::new(x, origin_y, z), Vector::new(0.0, -1.0, 0.0));
        let toi = collider
            .shape()
            .cast_ray(&Pose::IDENTITY, &ray, 1.0e5, true)
            .unwrap_or_else(|| panic!("terrain collider missed a vertical ray at ({x}, {z})"));
        origin_y - toi
    }

    #[test]
    fn triangular_interpolates_and_clamps_at_edges() {
        // 2x2 corners: (i=0,j=0)=0, (i=1,j=0)=255, (i=0,j=1)=0, (i=1,j=1)=255 — a pure x-ramp.
        // Planar data, so both cell triangles lie in the ramp plane: exact everywhere.
        let grid = grid_from(2, |i, _| if i == 1 { 255 } else { 0 });
        let h = WORLD_HALF_EXTENT;
        assert_eq!(grid.height_at(-h, 0.0), 0.0);
        assert!((grid.height_at(h, 0.0) - HEIGHT_RANGE).abs() < 1e-4);
        assert!((grid.height_at(0.0, 123.0) - HEIGHT_RANGE * 0.5).abs() < 1e-4);
        assert!((grid.height_at(h / 2.0, -h) - HEIGHT_RANGE * 0.75).abs() < 1e-4);
        // Beyond the map edge the surface continues flat (clamped).
        assert_eq!(grid.height_at(-h - 500.0, 0.0), 0.0);
        assert!((grid.height_at(h + 500.0, 9999.0) - HEIGHT_RANGE).abs() < 1e-4);

        // A genuinely non-planar cell: one raised corner (h11 = H), sample the interior. On the
        // anti-diagonal split, (fu, fv) = (0.75, 0.75) lies in the upper triangle
        // {h10, h11, h01}: H + (0 − H)·0.25 + (0 − H)·0.25 = 0.5·H — NOT bilinear's 0.5625·H,
        // and NOT a main-diagonal split's 0.75·H. This is the same point the collider test
        // below raycasts, so height_at and parry are pinned to the same answer.
        let hi = 200.0 / 255.0 * HEIGHT_RANGE;
        let corner = grid_from(2, |i, j| if i == 1 && j == 1 { 200 } else { 0 });
        assert!((corner.height_at(h / 2.0, h / 2.0) - hi * 0.5).abs() < 1e-4);
        // Dead center (fu = fv = 0.5) sits ON the split diagonal, where the raised corner
        // contributes nothing: exactly 0 (bilinear would say 0.25·H).
        assert!(corner.height_at(0.0, 0.0).abs() < 1e-4);
    }

    /// The classic transposed-heightfield bug, pinned empirically: build the PRODUCTION collider
    /// from ramps along each axis and raycast it with parry. An x-ramp must rise along +X (a
    /// transposed matrix would make it rise along +Z), the span must be the full world square,
    /// and heights must match `height_at` (planar ramps make triangle == bilinear exactly).
    #[test]
    fn heightfield_matches_grid_orientation_and_scale() {
        let h = WORLD_HALF_EXTENT;

        let x_ramp = grid_from(4, |i, _| (i * 85) as u8); // 0, 85, 170, 255 along x
        let collider = heightfield_collider(&x_ramp);
        for (x, z) in [(-h, 0.0), (h * 0.5, -h * 0.9), (0.0, h * 0.7), (h, h)] {
            let expected = x_ramp.height_at(x, z);
            let got = cast_down(&collider, x, z);
            assert!(
                (got - expected).abs() < 1e-3,
                "x-ramp collider at ({x}, {z}): cast {got}, grid {expected}"
            );
        }
        // The discriminator: 3/4 across +X must be 3/4 of the range, NOT the z-value (0.5·range).
        let got = cast_down(&collider, h * 0.5, 0.0);
        assert!(
            (got - HEIGHT_RANGE * 0.75).abs() < 1e-3,
            "x-ramp must rise along +X (transposed heightfield?): cast {got}"
        );

        let z_ramp = grid_from(4, |_, j| (j * 85) as u8);
        let collider = heightfield_collider(&z_ramp);
        let got = cast_down(&collider, 0.0, h * 0.5);
        assert!(
            (got - HEIGHT_RANGE * 0.75).abs() < 1e-3,
            "z-ramp must rise along +Z (transposed heightfield?): cast {got}"
        );
        let got = cast_down(&collider, -h * 0.8, -h * 0.5);
        let expected = z_ramp.height_at(-h * 0.8, -h * 0.5);
        assert!((got - expected).abs() < 1e-3, "z-ramp: {got} vs {expected}");
    }

    /// THE diagonal-convention pin (ONE-SURFACE invariant, module doc): a single raised corner
    /// makes the two candidate cell splits disagree hard in the cell interior, so a parry
    /// raycast there identifies the diagonal empirically. Parry's default split is the
    /// ANTI-diagonal (`(x_lo, z_hi)`–`(x_hi, z_lo)`); `height_at` must give the identical
    /// surface. If parry ever changes convention, this fails before anything subtle does.
    #[test]
    fn parry_splits_cells_along_the_anti_diagonal() {
        let h = WORLD_HALF_EXTENT;
        let hi = 200.0 / 255.0 * HEIGHT_RANGE;
        let corner = grid_from(2, |i, j| if i == 1 && j == 1 { 200 } else { 0 });
        let collider = heightfield_collider(&corner);
        // (fu, fv) = (0.75, 0.75): anti-diagonal → 0.5·H; main diagonal would read 0.75·H and
        // bilinear 0.5625·H — cleanly separated at H ≈ 117.6 m.
        let got = cast_down(&collider, h / 2.0, h / 2.0);
        assert!(
            (got - hi * 0.5).abs() < 1e-2,
            "parry cell split is not the anti-diagonal: cast {got}, expected {}",
            hi * 0.5
        );
        // Dead center lies ON the anti-diagonal: exactly between the two zero corners.
        let got = cast_down(&collider, 0.0, 0.0);
        assert!(got.abs() < 1e-2, "cell-center cast {got}, expected 0");
        // And both points agree with height_at — collider == oracle surface.
        for (x, z) in [(h / 2.0, h / 2.0), (0.0, 0.0), (-h / 3.0, h / 5.0)] {
            let cast = cast_down(&collider, x, z);
            let grid = corner.height_at(x, z);
            assert!(
                (cast - grid).abs() < 1e-2,
                "collider vs height_at at ({x}, {z}): {cast} vs {grid}"
            );
        }
    }

    /// The ONE-SURFACE tolerance pin: `height_at` vs a parry raycast of the production collider
    /// at many random-but-seeded interior points over a rough (seeded-noise) grid — the two
    /// must agree to a few MILLIMETRES everywhere (before this invariant the measured
    /// disagreement was up to 0.519 m, more than the suspension's 0.5 m probe reach).
    #[test]
    fn oracle_matches_collider_at_seeded_points() {
        // Deterministic LCG noise for the grid and the sample points (no platform RNG).
        let mut state: u64 = 0x243F_6A88_85A3_08D3; // seeded, fixed
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as u32) as f32 / u32::MAX as f32 // 0..1
        };
        let size = 33u32;
        let mut samples = Vec::with_capacity((size * size) as usize);
        for _ in 0..size * size {
            samples.push(next() * HEIGHT_RANGE);
        }
        let grid = HeightGrid::new(samples.into(), size);
        let collider = heightfield_collider(&grid);
        let span = WORLD_HALF_EXTENT * 0.98; // interior only (edge rays can graze the border)
        let mut worst = 0.0f32;
        for _ in 0..512 {
            let x = (next() * 2.0 - 1.0) * span;
            let z = (next() * 2.0 - 1.0) * span;
            let cast = cast_down(&collider, x, z);
            let analytic = grid.height_at(x, z);
            worst = worst.max((cast - analytic).abs());
        }
        // Measured on first run: worst 6.1e-5 m over these 512 points — the 3 mm pin is pure
        // headroom for parry/f32 arithmetic changes, not observed error.
        assert!(
            worst < 3e-3,
            "oracle-vs-collider disagreement {worst} m exceeds the few-mm pin"
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
        let h = WORLD_HALF_EXTENT;
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

    /// The embedded kernel really is the normalized Gaussian for `SMOOTH_KERNEL_SIGMA` — pins
    /// the constant pair together so changing σ without regenerating the table fails here (the
    /// runtime deliberately never calls `exp`, which is not bit-identical across platforms).
    #[test]
    fn smooth_kernel_matches_its_sigma() {
        let sigma = f64::from(SMOOTH_KERNEL_SIGMA);
        let raw: Vec<f64> = (0..SMOOTH_KERNEL.len())
            .map(|k| (-((k * k) as f64) / (2.0 * sigma * sigma)).exp())
            .collect();
        let sum = raw[0] + 2.0 * raw[1..].iter().sum::<f64>();
        for (k, &w) in SMOOTH_KERNEL.iter().enumerate() {
            let expected = raw[k] / sum;
            assert!(
                (f64::from(w) - expected).abs() < 1e-6,
                "kernel[{k}] = {w} but σ = {sigma} wants {expected}"
            );
        }
        assert_eq!(
            SMOOTH_KERNEL.len() - 1,
            (3.0 * sigma).ceil() as usize,
            "kernel radius must be 3σ"
        );
    }

    /// The de-terracing claim, measured: an 8-bit-quantized ramp (0.588 m staircase, ~4 px per
    /// step) smooths back to within a few cm of the ideal ramp away from the clamped edges.
    ///
    /// The reference includes the staircase's own DC bias (round-half-up puts the mean of this
    /// residual at +step/8, ~7 cm): quantization bias is information the blur cannot recover —
    /// the claim is that the ±0.294 m TERRACING flattens, not that the ≤ half-step offset
    /// vanishes.
    #[test]
    fn smoothing_flattens_a_quantized_staircase_onto_the_ideal_ramp() {
        let size = 128usize;
        let step = HEIGHT_RANGE / 255.0; // the 8-bit quantum, ~0.588 m
        let slope = step / 4.0; // one quantization step every ~4 px
        let ideal = |i: usize| i as f32 * slope;
        let quantized = |i: usize| (ideal(i) / step).round() * step;
        let mut samples = Vec::with_capacity(size * size);
        for _j in 0..size {
            for i in 0..size {
                // What an 8-bit export stores: the ideal ramp snapped to the quantum grid.
                samples.push(quantized(i));
            }
        }
        let dc = (0..size).map(|i| quantized(i) - ideal(i)).sum::<f32>() / size as f32;
        let smoothed = gaussian_smooth(&samples, size);
        let margin = SMOOTH_KERNEL.len(); // kernel radius + 1: clear of edge clamping
        let mut worst = 0.0f32;
        for j in margin..size - margin {
            for i in margin..size - margin {
                worst = worst.max((smoothed[j * size + i] - (ideal(i) + dc)).abs());
            }
        }
        assert!(
            worst < 0.03,
            "smoothed staircase deviates {worst:.3} m from the ideal ramp (quantum {step:.3} m)"
        );
    }

    /// The SHIPPED asset, decoded through the real path: 4096² 16-bit source, downsampled ONCE
    /// to [`GRID_RESOLUTION`]² (the ONE surface), using the full 0..150 m range. Also the LFS
    /// tripwire — a pointer file (~130 text bytes) fails the size guard here in CI instead of
    /// panicking at first boot on the droplet.
    #[test]
    fn shipped_heightmap_decodes_full_range_through_the_real_path() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(HEIGHT_MAP_PATH);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|err| panic!("shipped heightmap missing at {}: {err}", path.display()));
        assert!(
            bytes.len() > 1024,
            "{} is {} bytes — a Git LFS POINTER, not the map (checkout without lfs pull)",
            path.display(),
            bytes.len()
        );
        let grid = grid_from_png(&bytes).expect("shipped heightmap must decode");
        assert_eq!(
            grid.size(),
            GRID_RESOLUTION,
            "the decode must land at the ONE shared surface resolution"
        );
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        let step = WORLD_SIZE / 128.0;
        for j in 0..=128 {
            for i in 0..=128 {
                let h = grid.height_at(
                    -WORLD_HALF_EXTENT + i as f32 * step,
                    -WORLD_HALF_EXTENT + j as f32 * step,
                );
                lo = lo.min(h);
                hi = hi.max(h);
            }
        }
        assert!(lo >= 0.0 && hi <= HEIGHT_RANGE + 1e-3, "range [{lo}, {hi}]");
        assert!(
            hi - lo > 10.0,
            "the map should span real relief, got [{lo}, {hi}]"
        );
    }

    /// The render mesh is the grid's own surface (ONE-SURFACE invariant): vertices are the grid
    /// samples themselves, and every cell splits along the same anti-diagonal parry uses (the
    /// index pattern's shared edge is `i01`–`i10`).
    #[test]
    fn render_mesh_tiles_are_the_grid_surface_with_the_parry_diagonal() {
        let grid = grid_from(4, |i, j| (i * 40 + j * 20) as u8);
        let tiles = terrain_mesh_tiles(&grid);
        assert_eq!(
            tiles.len(),
            1,
            "3 cells fit one {MESH_TILE_CELLS}-cell tile"
        );
        let mesh = &tiles[0];
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("terrain mesh must carry f32x3 positions");
        };
        assert_eq!(positions.len(), 16, "vertices are the 4² grid nodes");
        for p in positions {
            let expected = grid.height_at(p[0], p[2]);
            assert!(
                (p[1] - expected).abs() < 1e-3,
                "mesh vertex {p:?} off the surface (expected y {expected})"
            );
        }
        // The first cell's six indices: triangles {i00, i01, i10} and {i10, i01, i11} share the
        // i01–i10 edge — the anti-diagonal ((x_lo, z_hi)–(x_hi, z_lo)), same as parry/height_at.
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("terrain mesh must carry u32 indices");
        };
        assert_eq!(&indices[..6], &[0u32, 4, 1, 1, 4, 5][..]);
    }

    /// Multi-tile chunking is culling-only: tiles cover every cell exactly once, border vertices
    /// are shared (same world position, same height), and every vertex still sits on the ONE
    /// surface.
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
        let grid = HeightGrid::new(samples.into(), size);
        let tiles = terrain_mesh_tiles(&grid);
        assert_eq!(tiles.len(), 4);
        let mut triangles = 0;
        for mesh in &tiles {
            let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("terrain mesh must carry f32x3 positions");
            };
            for p in positions {
                // Vertices must BE grid samples (not merely near the interpolated surface):
                // recover the node indices from the exact world position and compare raw.
                let step = WORLD_SIZE / (size - 1) as f32;
                let i = ((p[0] + WORLD_HALF_EXTENT) / step).round() as usize;
                let j = ((p[2] + WORLD_HALF_EXTENT) / step).round() as usize;
                let expected = grid.sample(i, j);
                assert_eq!(
                    p[1], expected,
                    "tile vertex {p:?} is not the grid sample at node ({i}, {j})"
                );
            }
            let Some(Indices::U32(indices)) = mesh.indices() else {
                panic!("terrain mesh must carry u32 indices");
            };
            triangles += indices.len() / 3;
        }
        assert_eq!(
            triangles,
            130 * 130 * 2,
            "tiles must cover every cell exactly once"
        );
    }
}
