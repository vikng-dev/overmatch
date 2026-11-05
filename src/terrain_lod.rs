//! Terrain render LOD: error-bounded triangle ladders for the 64 render tiles the heightmap world
//! already draws (`terrain_grid::terrain_mesh_tiles`), selected declaratively by bevy's
//! [`VisibilityRange`].
//!
//! Each tile is a DERIVED 129² patch (129 = 2⁷ + 1; [`MESH_TILE_CELLS`] and `GRID_RESOLUTION`
//! satisfy RTIN's 2ⁿ+1 constraint). Build its RTIN / Martini error pyramid once, then extract
//! [`TERRAIN_LOD_LADDER`] rungs. Deviation δ bounds the mesh everywhere; triangle count follows,
//! never drives it. CLAIMED examples: a flat perimeter collapses to a handful at 2 cm; a ridge
//! stays dense until 25 cm.
//!
//! # THE THREE INVARIANTS
//!
//! 1. **ONE-SURFACE** (`terrain_grid`): generate at Startup from the same [`HeightGrid`] as
//!    oracle, collider and spawns. No baked product can go stale. Re-arm a versioned manifest if
//!    generation becomes a build step for streaming or a 5 km map.
//!    Kept vertices are exact samples (`interior_vertices_are_exact_grid_samples`), which alone
//!    does not certify the surface: RTIN changes connectivity from canonical anti-diagonals to
//!    main diagonals. [`rtin::Rtin::canonicalize_cell_diagonals`] re-splits fully refined cells;
//!    [`worst_deviation_m`] measures the continuous maximum over both triangulations' overlay.
//!
//! 2. **NO SKIRTS.** Full-density borders match vertex-for-vertex at every tile/level pairing:
//!    no neighbour state, 16 edge-index variants or metre-class curtains casting grid shadows
//!    under the 17° sun. [`rtin::Rtin::error_pyramid`] enforces borders before extraction; post-
//!    processing would break restricted-bintree agreement and crack tile interiors.
//!
//! 3. **Shared discrete thresholds.** Quantize each level's MEASURED deviation upward to the
//!    smallest containing rung; switch distance depends only on (rung, view profile).
//!    `bevy_render-0.19.0/src/view/visibility/range.rs` retains range slots for app lifetime and
//!    indexes them with `u16`: per-tile floats eventually leak into slot-zero aliasing. Shared
//!    rungs bound this to a couple of dozen distinct values regardless of tile count.
//!
//! # THE DOCTRINE, AND WHAT IT DELIBERATELY DOES NOT COVER
//!
//! As with model LOD, the sole gate is worst-case surface deviation projected under the live
//! view's pixel budget (`every_lod_level_stays_within_its_declared_deviation` plus rung switches).
//! The following channels escape that bound. Deliberately no extra gate, threshold or held-back
//! rung: a shared bound avoids per-channel rules. MEASURED on the shipped map, 2026-08-02.
//!
//! 1. **Shadows.** `terrain_shadow`'s baked field reads the canonical grid; terrain levels never
//!    cast into cascades, so LOD cannot move the field. They receive at their own height:
//!    DERIVED on a flat receiver, δh moves the edge by δh·cot 17° ≈ 3.27 δh.
//!
//! 2. **Grazing-LOS first-hit slide.** Projected error cannot see motion ALONG a pixel ray. Near
//!    tangency the intersection satisfies `δt ≈ δh / (d_y − ∇h·d_xz)`, whose denominator goes to
//!    zero exactly where tanks look at each other. MEASURED worst first-hit displacement over 1146
//!    paired grazing hits: 0.000 m in the gunner optic (no coarse level is ever selected there on a
//!    1 km map), 0.507 m in the commander view at the shipped 1 px budget, and 166.098 m at an 8 px
//!    budget. Mostly DEPTH displacement — the hit slides toward or away from the eye along the line
//!    of sight, where it subtends almost no angle — which is why metres of it can be invisible.
//!
//! 3. **Crest occlusion.** MEASURED over 753 hull-height sightlines per profile: zero flips in
//!    either direction at 1 px, and one sightline where the coarse ground REVEALS a hull the exact
//!    ground hides at 8 px. A budget sweep at the commander field put the transition between 4 px
//!    (zero) and 5 px (one). Sub-pixel doctrine deliberately does not gate this — re-measure if it
//!    ever matters.
//!
//!
//! # The landmine this module is built around
//!
//! `check_visibility_ranges` (`bevy_camera-0.19.0/src/visibility/range.rs:263`) measures to
//! `entity_transform.translation()` — world origin for world-space tiles at identity — unless
//! `use_aabb` selects `affine.transform_point3a(aabb.center)`. This is the centre, not the nearest
//! point: pin the same exact-level [`Aabb`] on every level, and add tile bounding radius to
//! switch distances to protect the nearest surface.
//!
//! Tile-local vertices buy no different anchor: identity plus world-space bounds equals a
//! tile-centre transform plus local bounds. Keep world-space vertices so `terrain_mesh_tiles`
//! and its exact-grid-sample assertions share one layout.
//!
//! `NoAutoAabb` is not optional either: bevy's `calculate_bounds` re-derives the AABB on
//! `Changed<Mesh3d>` (`bevy_camera-0.19.0/src/visibility/mod.rs:568`), which fires on spawn and
//! would replace our pinned bounds with each LEVEL's own — drifting the anchor per level and
//! opening sub-metre gaps and overlaps in the range chain.

use std::time::Instant;

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::{NoAutoAabb, VisibilityRange};
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;

use crate::render_policy::VisualScope;
use crate::terrain_grid::{
    HeightGrid, MESH_TILE_CELLS, TERRAIN_MESH_USAGE, TEXTURE_TILE_M, mesh_tile_node_ranges,
    node_world_coord, surface_normal_at,
};
use crate::view::{ViewFacts, ViewProfile};

/// Declared metre deviations, both extraction schedule and upward-quantized rung vocabulary.
/// Each kept mesh lies within its rung everywhere, not just at nodes. Rung 0 uses the exact
/// [`crate::terrain_grid::terrain_mesh_tiles`] surface.
///
/// MEASURED delivered set on the shipped map: {0, 0.05, 0.10, 0.25, 0.50} m (ratified).
/// The 2 cm extraction earns no rung itself, but produces some tiles' 5 cm levels; deleting it
/// loses those levels. One extra extraction preserves them. Tiles keep only distinct meshes.
pub(crate) use crate::camera::TERRAIN_LOD_LADDER;

/// Pixel budget below which a level's deviation may project before its finer neighbour takes over.
/// Fixed separately from model LOD settings: terrain error can hide or expose a hull at a crest.
/// `wired_thresholds_are_the_derivation` catches literal thresholds or unpropagated budget changes.
pub(crate) use crate::camera::TERRAIN_LOD_BUDGET_PX;

/// Shared live view facts at this ladder's [`TERRAIN_LOD_BUDGET_PX`]. Compose rather than store
/// a profile, so its facts cannot drift from the tank chains' selection view.
pub(crate) fn terrain_view(facts: ViewFacts) -> ViewProfile {
    ViewProfile::of(facts, TERRAIN_LOD_BUDGET_PX)
}

/// Camera-to-tile-centre metres at which `rung` becomes legal; rung 0 starts at the camera.
/// Exact projection ([`ViewFacts::sub_pixel_distance_m`]) plus radius protects the nearest
/// surface because `use_aabb` measures to the centre (source pin in module doc). Use the largest
/// tile radius for every tile: conservative without per-tile floats leaking range-table slots.
/// Free function because spawning mutably borrows the ladder's radius; the profile is composed.
pub(crate) fn rung_switch_distance_m(view: ViewProfile, rung: usize, radius_m: f32) -> f32 {
    if rung == 0 {
        return 0.0;
    }
    view.switch_distance_m(TERRAIN_LOD_LADDER[rung], radius_m)
}

pub(crate) struct TerrainLodLevel {
    /// Index into [`TERRAIN_LOD_LADDER`] this level's MEASURED deviation was quantized up to. THIS
    /// is what the switch distance is computed from, and `measured_dev_m ≤ TERRAIN_LOD_LADDER[rung]`
    /// always holds.
    pub(crate) rung: usize,
    /// Extraction threshold, a heuristic, not the guarantee. MEASURED: the pyramid under-reports
    /// continuous deviation by up to 15.9× on the shipped map. Errors compound down levels, and
    /// node measurements miss between-node differences. Martini/Cesium use it as a heuristic;
    /// [`Self::measured_dev_m`]'s exact continuous maximum determines the declared rung instead.
    pub(crate) extracted_at_m: f32,
    /// The EXACT continuous maximum of |this level − the canonical surface| over the whole tile,
    /// metres ([`worst_deviation_m`]) — every point, not every node. Always ≤ the declared rung;
    /// that is the invariant the switch distance rests on.
    pub(crate) measured_dev_m: f32,
    pub(crate) triangles: usize,
    /// The renderable mesh: world-space positions, full-grid central-difference normals, world-XZ
    /// UVs and (after spawn) a mikktspace tangent basis.
    pub(crate) mesh: Mesh,
}

pub(crate) struct TerrainLodTile {
    /// Exact-level minimum corner of the tile, world space.
    pub(crate) min: Vec3,
    /// Exact-level maximum corner of the tile, world space. Pinned (with `min`) onto EVERY level of
    /// the tile so all of them share one anchor (see the module doc's landmine section).
    pub(crate) max: Vec3,
    /// Kept levels, finest first, strictly increasing in rung. Dominated and duplicate levels are
    /// already dropped.
    pub(crate) levels: Vec<TerrainLodLevel>,
}

impl TerrainLodTile {
    /// Half the diagonal of the tile's bounds — the worst-case distance from the AABB centre
    /// (what `use_aabb` measures to) to the tile's nearest surface point.
    pub(crate) fn bounding_radius_m(&self) -> f32 {
        (self.max - self.min).length() * 0.5
    }
}

pub(crate) struct TerrainLod {
    /// One entry per render tile, in `mesh_tile_node_ranges` order.
    pub(crate) tiles: Vec<TerrainLodTile>,
    /// Largest tile radius: shared conservative slack without per-tile threshold floats.
    pub(crate) bounding_radius_m: f32,
    /// Wall time of the whole generation, for the startup census.
    pub(crate) build: std::time::Duration,
}

impl TerrainLod {
    /// The complementary visibility ranges for one tile: `[0, s₁) [s₁, s₂) … [sₙ, ∞)`, every
    /// distance covered exactly once, so a tile is never drawn twice and never vanishes.
    pub(crate) fn ranges(&self, tile: &TerrainLodTile, view: ViewProfile) -> Vec<VisibilityRange> {
        let starts: Vec<f32> = tile
            .levels
            .iter()
            .map(|level| rung_switch_distance_m(view, level.rung, self.bounding_radius_m))
            .collect();
        (0..starts.len())
            .map(|k| {
                let start = starts[k];
                let end = starts.get(k + 1).copied().unwrap_or(f32::INFINITY);
                VisibilityRange {
                    start_margin: start..start,
                    end_margin: end..end,
                    // Use the pinned centre; world-space tiles' transforms sit at world origin.
                    use_aabb: true,
                }
            })
            .collect()
    }
}

/// Marker on every spawned terrain LOD entity, carrying the coordinates the adaptive layer needs
/// to rewrite its range without regenerating any geometry.
#[derive(Component, Clone, Copy)]
pub(crate) struct TerrainLodEntity {
    /// Index into [`TerrainLod::tiles`].
    pub(crate) tile: usize,
    /// Position of this level within its tile's kept ladder (0 = finest).
    pub(crate) level: usize,
}

// ---------------------------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------------------------

/// Build from the decoded grid: exact rung 0 uses [`crate::terrain_grid::terrain_mesh_tiles`];
/// reduced rungs use RTIN. Negative-threshold extraction has identical canonical connectivity
/// (`rtin_at_zero_error_reproduces_the_canonical_tessellation`).
/// Only full-sized tiles use the shared 2ⁿ+1 hierarchy; smaller/re-authored fixture tiles keep
/// their exact level rather than attempting an invalid triangulation.
pub(crate) fn build(grid: &HeightGrid) -> TerrainLod {
    let started = Instant::now();
    let ranges = mesh_tile_node_ranges(grid);
    let base = crate::terrain_grid::terrain_mesh_tiles(grid);
    assert_eq!(
        ranges.len(),
        base.len(),
        "the LOD tiling must be the render tiling"
    );
    // One hierarchy for the whole map: every full tile is the same 2ⁿ+1 patch shape, so the
    // right-triangle coordinate table is built once and shared by all 64 tiles.
    let full = MESH_TILE_CELLS + 1;
    let hierarchy = rtin::Rtin::new(full);

    let mut tiles = Vec::with_capacity(base.len());
    for (mesh0, [ia, ib, ja, jb]) in base.into_iter().zip(ranges) {
        let (w, h) = (ib - ia + 1, jb - ja + 1);
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut heights = Vec::with_capacity(w * h);
        for j in ja..=jb {
            for i in ia..=ib {
                let (x, y, z) = (
                    node_world_coord(grid, i),
                    grid.sample(i, j),
                    node_world_coord(grid, j),
                );
                heights.push(y);
                min = min.min(Vec3::new(x, y, z));
                max = max.max(Vec3::new(x, y, z));
            }
        }
        let mut levels = vec![TerrainLodLevel {
            rung: 0,
            extracted_at_m: -1.0,
            measured_dev_m: 0.0,
            triangles: mesh0.indices().map_or(0, |i| i.len() / 3),
            mesh: mesh0,
        }];
        if w == full && h == full {
            let errors = hierarchy.error_pyramid(&heights);
            let mut previous: Option<Vec<[u32; 3]>> = None;
            for &threshold in &TERRAIN_LOD_LADDER[1..] {
                let tris = hierarchy.triangulate(&errors, threshold);
                // Duplicate meshes buy no triangles but cost a range slot and draw bookkeeping
                // (a flat perimeter tile may already be minimal at 2 cm).
                if previous.as_ref().is_some_and(|p| *p == tris) {
                    continue;
                }
                previous = Some(tris.clone());
                let measured = worst_deviation_m(&heights, full, &tris);
                // Quantize the continuous measurement, not the heuristic threshold; see
                // `TerrainLodLevel::extracted_at_m`. No multiplier, including the ~1.6× estimate,
                // replaces that measurement. DERIVED examples: extracted at 5 cm, measured
                // 7.7 cm → rung 10 cm; extracted at 50 cm, measured 3 cm → rung 5 cm. Either
                // direction is safe only because the measured bound determines switch distance.
                let Some(quantized) = TERRAIN_LOD_LADDER
                    .iter()
                    .position(|&declared| measured <= declared)
                else {
                    // Off the top of the ladder: there is no rung honest enough to declare, so
                    // this level does not exist. A rough tile simply stops getting coarser.
                    continue;
                };
                let kept = levels.last_mut().expect("rung 0 always exists");
                if quantized <= kept.rung {
                    // Lands on a rung we already keep (the measurement is not monotone in the
                    // threshold): take the cheaper mesh, leave the ladder's rung set unchanged.
                    if quantized == kept.rung && tris.len() < kept.triangles {
                        kept.extracted_at_m = threshold;
                        kept.triangles = tris.len();
                        kept.measured_dev_m = measured;
                        kept.mesh = tile_mesh(grid, ia, ja, full, &tris);
                    }
                    continue;
                }
                levels.push(TerrainLodLevel {
                    rung: quantized,
                    extracted_at_m: threshold,
                    measured_dev_m: measured,
                    triangles: tris.len(),
                    mesh: tile_mesh(grid, ia, ja, full, &tris),
                });
            }
        }
        tiles.push(TerrainLodTile { min, max, levels });
    }
    let bounding_radius_m = tiles
        .iter()
        .map(TerrainLodTile::bounding_radius_m)
        .fold(0.0f32, f32::max);
    TerrainLod {
        tiles,
        bounding_radius_m,
        build: started.elapsed(),
    }
}

/// RTIN indices → world-space grid samples, [`surface_normal_at`] full-grid shading and shared
/// world-XZ UVs. Recomputing normals from simplified triangles would pop shading with geometry.
fn tile_mesh(grid: &HeightGrid, ia: usize, ja: usize, size: usize, tris: &[[u32; 3]]) -> Mesh {
    let mut remap = vec![u32::MAX; size * size];
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::with_capacity(tris.len() * 3);
    for tri in tris {
        for &node in tri {
            let slot = &mut remap[node as usize];
            if *slot == u32::MAX {
                let (u, v) = (node as usize % size, node as usize / size);
                let (i, j) = (ia + u, ja + v);
                let (x, z) = (node_world_coord(grid, i), node_world_coord(grid, j));
                *slot = positions.len() as u32;
                positions.push([x, grid.sample(i, j), z]);
                normals.push(surface_normal_at(grid, i, j));
                uvs.push([x / TEXTURE_TILE_M, z / TEXTURE_TILE_M]);
            }
            indices.push(*slot);
        }
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, TERRAIN_MESH_USAGE);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Exact continuous maximum |LOD − canonical surface| over a tile, metres.
///
/// Node agreement is insufficient: RTIN's alternating bintree uses main diagonals for roughly
/// half its leaves; [`HeightGrid::height_at`] and parry use anti-diagonals. At a cell's centre
/// these differ by `|h₀₀ + h₁₁ − h₁₀ − h₀₁| / 2` despite identical corners: MEASURED 0.323 m in a
/// shipped cell, twelve times its node-sampled error.
///
/// The difference is piecewise linear on the triangulations' overlay, so its maximum occurs at
/// an overlay vertex. Both meshes' vertices are grid nodes. LOD edges run along grid lines or
/// at 45° through nodes: axis edges meet canonical anti-diagonals only at node endpoints, and
/// 45° edges cross grid lines only at nodes. Within a cell they coincide with its anti-diagonal
/// or cross it on the main diagonal at the centre. Thus nodes plus centres contain every overlay
/// vertex; extra centres inside a linear region are redundant but safe. One exact pass needs no
/// sampling parameter and cannot alias past a maximum.
///
/// Doubled coordinates put nodes `(2i, 2j)` and centres `(2i+1, 2j+1)` on the same-parity integer
/// lattice for exact inside tests. On the anti-diagonal, a centre's canonical height is the mean
/// of its two endpoints.
pub(crate) fn worst_deviation_m(heights: &[f32], size: usize, tris: &[[u32; 3]]) -> f32 {
    let n = size as i64;
    let node = |k: u32| (i64::from(k) % n, i64::from(k) / n);
    let height = |i: i64, j: i64| f64::from(heights[(j * n + i) as usize]);
    let canonical = |p: i64, q: i64| -> f64 {
        if p & 1 == 0 {
            height(p / 2, q / 2)
        } else {
            let (i, j) = (p >> 1, q >> 1);
            (height(i + 1, j) + height(i, j + 1)) * 0.5
        }
    };
    let mut worst = 0.0f64;
    for tri in tris {
        let ((ax, ay), (bx, by), (cx, cy)) = (node(tri[0]), node(tri[1]), node(tri[2]));
        let (ha, hb, hc) = (height(ax, ay), height(bx, by), height(cx, cy));
        let (ax, ay, bx, by, cx, cy) = (ax * 2, ay * 2, bx * 2, by * 2, cx * 2, cy * 2);
        let area2 = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
        if area2 == 0 {
            continue;
        }
        let inv = 1.0 / area2 as f64;
        let (x0, x1) = (ax.min(bx).min(cx), ax.max(bx).max(cx));
        let (y0, y1) = (ay.min(by).min(cy), ay.max(by).max(cy));
        for q in y0..=y1 {
            // Same-parity points only: even/even are nodes, odd/odd are cell centres. Mixed parity
            // is an edge midpoint, which is never an overlay vertex.
            let mut p = x0 + ((x0 ^ q) & 1);
            while p <= x1 {
                // Inside signs match `area2`; include zero so both triangles cover shared edges
                // with the same surface value.
                let ea = (cx - bx) * (q - by) - (cy - by) * (p - bx);
                let eb = (ax - cx) * (q - cy) - (ay - cy) * (p - cx);
                let ec = (bx - ax) * (q - ay) - (by - ay) * (p - ax);
                let inside = if area2 > 0 {
                    ea >= 0 && eb >= 0 && ec >= 0
                } else {
                    ea <= 0 && eb <= 0 && ec <= 0
                };
                if inside {
                    let plane = (ea as f64 * ha + eb as f64 * hb + ec as f64 * hc) * inv;
                    worst = worst.max((plane - canonical(p, q)).abs());
                }
                p += 2;
            }
        }
    }
    worst as f32
}

// ---------------------------------------------------------------------------------------------
// RTIN / Martini
// ---------------------------------------------------------------------------------------------

/// Right-Triangulated Irregular Networks (Evans/Kirkpatrick/Townsend), in Mapbox's `martini`
/// formulation. Ported (~120 lines, MIT) from the algorithm rather than taken as a dependency —
/// `bevy_rtin` is a faithful but unstarred 533-line crate and this is the whole of what we need.
///
/// Pure and deterministic: integer coordinate arithmetic and f32 comparisons only, no allocation
/// order dependence, no transcendentals. The same grid produces the same triangle list on every
/// machine.
pub(crate) mod rtin {
    /// The right-triangle hierarchy over a `size`² node grid, `size = 2ⁿ + 1`.
    ///
    /// Only the SPLITTABLE triangles are enumerated: a leaf triangle is half a grid cell, its
    /// hypotenuse is a cell diagonal, and the midpoint of that diagonal is the cell CENTRE — not a
    /// grid node, so there is no error to record and nothing below it to split. That is why the
    /// count is `2·cells² − 2` and not the full binary forest's `4·cells² − 2`.
    pub(crate) struct Rtin {
        /// Nodes per side of the patch this hierarchy triangulates.
        pub(crate) size: usize,
        /// `[ax, ay, bx, by]` per splittable triangle — the hypotenuse endpoints. The right-angle
        /// vertex `c` is derived from them (`cx = mx + my − ay`, `cy = my + ax − mx`).
        coords: Vec<u16>,
        triangles: usize,
        /// Triangles `[0, parents)` have splittable children whose recorded errors must be
        /// max-folded into them; the rest bottom out on leaves.
        parents: usize,
    }

    impl Rtin {
        /// Build the hierarchy for a `size`² grid. `size − 1` must be a power of two.
        pub(crate) fn new(size: usize) -> Self {
            let cells = size - 1;
            assert!(
                cells.is_power_of_two(),
                "RTIN needs a 2ⁿ+1 grid, got {size}"
            );
            let triangles = cells * cells * 2 - 2;
            let parents = triangles - cells * cells;
            let mut coords = vec![0u16; triangles * 4];
            let cells = cells as i32;
            for i in 0..triangles {
                let mut id = i + 2;
                let (mut ax, mut ay, mut bx, mut by, mut cx, mut cy) = (0i32, 0, 0, 0, 0, 0);
                if id & 1 != 0 {
                    bx = cells;
                    by = cells;
                    cx = cells;
                } else {
                    ax = cells;
                    ay = cells;
                    cy = cells;
                }
                loop {
                    id >>= 1;
                    if id <= 1 {
                        break;
                    }
                    let (mx, my) = ((ax + bx) >> 1, (ay + by) >> 1);
                    if id & 1 != 0 {
                        // Left half: the parent's right-angle vertex becomes the new `a`.
                        bx = ax;
                        by = ay;
                        ax = cx;
                        ay = cy;
                    } else {
                        // Right half.
                        ax = bx;
                        ay = by;
                        bx = cx;
                        by = cy;
                    }
                    cx = mx;
                    cy = my;
                }
                let k = i * 4;
                coords[k] = ax as u16;
                coords[k + 1] = ay as u16;
                coords[k + 2] = bx as u16;
                coords[k + 3] = by as u16;
            }
            Self {
                size,
                coords,
                triangles,
                parents,
            }
        }

        /// Per-node midpoint-error heuristic, in height units; not a continuous-deviation bound
        /// (see `TerrainLodLevel::extracted_at_m`). Bottom-up max-folding of own/child errors is
        /// monotone: hypotenuse neighbours read the same entry and therefore split together.
        ///
        /// Seed non-corner border nodes to infinity before folding. Each is a diamond midpoint
        /// (or an unpaired border triangle's), forcing splits at every threshold and propagating
        /// infinity through ancestors. This refines only the graded band reaching the full-density
        /// borders. Changing the pyramid preserves restricted-bintree agreement inside the tile;
        /// post-processing extracted splits instead would open interior T-junctions.
        pub(crate) fn error_pyramid(&self, heights: &[f32]) -> Vec<f32> {
            let size = self.size;
            assert_eq!(
                heights.len(),
                size * size,
                "error pyramid needs a size² patch"
            );
            let mut errors = vec![0.0f32; size * size];
            let last = size - 1;
            for k in 1..last {
                errors[k] = f32::INFINITY; // bottom edge
                errors[last * size + k] = f32::INFINITY; // top edge
                errors[k * size] = f32::INFINITY; // left edge
                errors[k * size + last] = f32::INFINITY; // right edge
            }
            let n = size as i64;
            for i in (0..self.triangles).rev() {
                let k = i * 4;
                let (ax, ay) = (i64::from(self.coords[k]), i64::from(self.coords[k + 1]));
                let (bx, by) = (i64::from(self.coords[k + 2]), i64::from(self.coords[k + 3]));
                let (mx, my) = ((ax + bx) >> 1, (ay + by) >> 1);
                let (cx, cy) = (mx + my - ay, my + ax - mx);
                let middle = (my * n + mx) as usize;
                let interpolated =
                    (heights[(ay * n + ax) as usize] + heights[(by * n + bx) as usize]) * 0.5;
                let error = (interpolated - heights[middle]).abs();
                errors[middle] = errors[middle].max(error);
                if i < self.parents {
                    let left = (((ay + cy) >> 1) * n + ((ax + cx) >> 1)) as usize;
                    let right = (((by + cy) >> 1) * n + ((bx + cx) >> 1)) as usize;
                    errors[middle] = errors[middle].max(errors[left]).max(errors[right]);
                }
            }
            errors
        }

        /// Extract at the heuristic `max_error`, returning patch node indices `v * size + u`.
        /// Certify continuous deviation separately with `worst_deviation_m`.
        /// A negative threshold splits every nonnegative-error triangle: DERIVED `2 · cells²`
        /// triangles, canonicalized to the exact `terrain_mesh_tiles` set. Equality of connectivity,
        /// not only count, is the sanity gate. Zero differs: a flat region has zero error and collapses.
        pub(crate) fn triangulate(&self, errors: &[f32], max_error: f32) -> Vec<[u32; 3]> {
            let last = (self.size - 1) as i32;
            let mut raw: Vec<[(i32, i32); 3]> = Vec::new();
            self.split(errors, max_error, &mut raw, (0, 0), (last, last), (last, 0));
            self.split(errors, max_error, &mut raw, (last, last), (0, 0), (0, last));
            self.canonicalize_cell_diagonals(&mut raw);
            let size = self.size as i32;
            let index = |p: (i32, i32)| (p.1 * size + p.0) as u32;
            raw.into_iter()
                .map(|[a, b, c]| {
                    // Counter-clockwise seen from +Y, the winding `terrain_mesh_tiles` emits and
                    // the one bevy's default face culling keeps. In grid coordinates (x = i,
                    // z = j) that is `uz·vx − ux·vz > 0` for the two edge vectors out of `a`.
                    let (u, v) = ((b.0 - a.0, b.1 - a.1), (c.0 - a.0, c.1 - a.1));
                    if u.1 * v.0 - u.0 * v.1 > 0 {
                        [index(a), index(b), index(c)]
                    } else {
                        [index(a), index(c), index(b)]
                    }
                })
                .collect()
        }

        /// Re-split fully refined cells along the canonical anti-diagonal. RTIN's structural
        /// leaf parity cannot be constrained to that diagonal; see `worst_deviation_m` for the
        /// half-twist error and MEASURED 0.323 m counterexample invisible at nodes.
        /// Two leaf halves share four corners and boundary edges, so changing only the interior
        /// diagonal preserves count, neighbours and crack-freeness. A leaf beside a coarser flank
        /// cannot be re-split locally; keep its error for continuous certification and rung quantization.
        fn canonicalize_cell_diagonals(&self, raw: &mut [[(i32, i32); 3]]) {
            let cells = self.size - 1;
            let mut halves = vec![[u32::MAX; 2]; cells * cells];
            for (index, &[a, b, c]) in raw.iter().enumerate() {
                // A leaf is half a grid cell: its `a`→`c` leg is one unit long in the L1 sense.
                if (a.0 - c.0).abs() + (a.1 - c.1).abs() != 1 {
                    continue;
                }
                let i = a.0.min(b.0).min(c.0) as usize;
                let j = a.1.min(b.1).min(c.1) as usize;
                let slot = &mut halves[j * cells + i];
                if slot[0] == u32::MAX {
                    slot[0] = index as u32;
                } else {
                    slot[1] = index as u32;
                }
            }
            for (cell, slot) in halves.iter().enumerate() {
                if slot[1] == u32::MAX {
                    continue; // not fully refined — the other half belongs to a coarser triangle
                }
                let (i, j) = ((cell % cells) as i32, (cell / cells) as i32);
                // Both halves share the cell's diagonal as their hypotenuse `a`–`b`; checking one
                // identifies it. The anti-diagonal joins (i, j+1) and (i+1, j).
                let [a, b, _] = raw[slot[0] as usize];
                let anti = [(i, j + 1), (i + 1, j)];
                if anti.contains(&a) && anti.contains(&b) {
                    continue;
                }
                // Canonical halves in the `(a, b, c)` shape whose winding fix matches
                // `terrain_mesh_tiles`' `[i00, i01, i10]` / `[i10, i01, i11]`.
                raw[slot[0] as usize] = [(i, j), (i + 1, j), (i, j + 1)];
                raw[slot[1] as usize] = [(i + 1, j), (i + 1, j + 1), (i, j + 1)];
            }
        }

        fn split(
            &self,
            errors: &[f32],
            max_error: f32,
            out: &mut Vec<[(i32, i32); 3]>,
            a: (i32, i32),
            b: (i32, i32),
            c: (i32, i32),
        ) {
            let size = self.size as i32;
            let (mx, my) = ((a.0 + b.0) >> 1, (a.1 + b.1) >> 1);
            // A leaf is half a grid cell: its `a`→`c` leg is one unit long in the L1 sense.
            let splittable = (a.0 - c.0).abs() + (a.1 - c.1).abs() > 1;
            if splittable && errors[(my * size + mx) as usize] > max_error {
                self.split(errors, max_error, out, c, a, (mx, my));
                self.split(errors, max_error, out, b, c, (mx, my));
                return;
            }
            out.push([a, b, c]);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Spawning + the adaptive layer
// ---------------------------------------------------------------------------------------------

/// OS failures before terrain mesh publication.
#[derive(Debug)]
enum TangentStartupError {
    Parallelism(std::io::Error),
    Worker(std::io::Error),
}

impl std::fmt::Display for TangentStartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parallelism(error) => {
                write!(f, "cannot discover terrain tangent worker count: {error}")
            }
            Self::Worker(error) => write!(f, "cannot start a terrain tangent worker: {error}"),
        }
    }
}

impl std::error::Error for TangentStartupError {}

/// Require mikktspace tangents at every rung (ADR-0011); without a basis, normal-map directions
/// have no frame and terrain renders flat/greasy. Parallelize per-mesh stateless work without
/// changing tangent order or values. MEASURED shipped map: 3.19 s single-threaded on 3.83 M pyramid
/// triangles versus 134 ms for generation, so tangents dominate startup cost.
fn generate_tangents(
    lod: &mut TerrainLod,
    parallelism: impl FnOnce() -> std::io::Result<std::num::NonZeroUsize>,
) -> Result<(), TangentStartupError> {
    let mut pending: Vec<(usize, usize, &mut Mesh)> = lod
        .tiles
        .iter_mut()
        .enumerate()
        .flat_map(|(tile, levels)| {
            levels
                .levels
                .iter_mut()
                .enumerate()
                .map(move |(level, entry)| (tile, level, &mut entry.mesh))
        })
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    let threads = parallelism()
        .map_err(TangentStartupError::Parallelism)?
        .get()
        .min(pending.len());
    let chunk = pending.len().div_ceil(threads);
    std::thread::scope(|scope| {
        for batch in pending.chunks_mut(chunk) {
            std::thread::Builder::new()
                .spawn_scoped(scope, move || {
                    for (tile, level, mesh) in batch {
                        mesh.generate_tangents().unwrap_or_else(|err| {
                            panic!(
                                "terrain LOD tile {tile} level {level} failed mikktspace tangent \
                             generation: {err}"
                            )
                        });
                    }
                })
                .map_err(TangentStartupError::Worker)?;
        }
        Ok(())
    })
}

/// Build, spawn and log the census for windowed `world::spawn_environment`.
/// At Startup the declared camera may not have supplied a view: `None` gives exact levels every
/// distance and coarser levels none, until [`adapt_ranges`] gets facts. A seeded profile would
/// guess the player's field and screen; source detail is conservatively over-detailed.
///
/// Every level carries `scope`: [`VisualScope::BAKED_CASTER`] for `terrain_shadow`'s baked field
/// (no cascade casting), or [`VisualScope::WORLD_SOLID`] for cascades.
pub(crate) fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: &Handle<crate::terrain_blend::TerrainMaterial>,
    grid: &HeightGrid,
    view: Option<ViewProfile>,
    scope: VisualScope,
) {
    let mut lod = build(grid);
    let tangents_started = Instant::now();
    generate_tangents(&mut lod, std::thread::available_parallelism).unwrap_or_else(|error| {
        error!("terrain startup refused before mesh publication: {error}");
        std::process::exit(1);
    });
    let tangents = tangents_started.elapsed();
    let mut census = vec![(0usize, 0usize, 0.0f32); TERRAIN_LOD_LADDER.len()];
    let mut entities = 0usize;
    let radius = lod.bounding_radius_m;
    for (tile_index, tile) in lod.tiles.iter_mut().enumerate() {
        let aabb = Aabb::from_min_max(tile.min, tile.max);
        // With no view yet, the finest level starts at zero and every coarser one at infinity —
        // the exact-surface tile owns every distance and its reductions own none.
        let starts: Vec<f32> = match view {
            Some(view) => tile
                .levels
                .iter()
                .map(|level| rung_switch_distance_m(view, level.rung, radius))
                .collect(),
            None => (0..tile.levels.len())
                .map(|level| if level == 0 { 0.0 } else { f32::INFINITY })
                .collect(),
        };
        for (level_index, level) in tile.levels.iter_mut().enumerate() {
            let (tiles_kept, tris, worst) = &mut census[level.rung];
            *tiles_kept += 1;
            *tris += level.triangles;
            *worst = worst.max(level.measured_dev_m);
            // Move tangented geometry; retain only the census. DERIVED: a second resident copy
            // of the whole pyramid would cost ~200 MB.
            let mesh = std::mem::replace(
                &mut level.mesh,
                Mesh::new(PrimitiveTopology::TriangleList, TERRAIN_MESH_USAGE),
            );
            let start = starts[level_index];
            let end = starts
                .get(level_index + 1)
                .copied()
                .unwrap_or(f32::INFINITY);
            commands.spawn((
                Transform::IDENTITY,
                Mesh3d(meshes.add(mesh)),
                crate::render_cost::CasterClass::Terrain,
                scope,
                MeshMaterial3d(material.clone()),
                // Pin identical bounds; `NoAutoAabb` protects the anchor (module source pin).
                aabb,
                NoAutoAabb,
                VisibilityRange {
                    start_margin: start..start,
                    end_margin: end..end,
                    use_aabb: true,
                },
                TerrainLodEntity {
                    tile: tile_index,
                    level: level_index,
                },
            ));
            entities += 1;
        }
    }
    for (rung, (tiles_kept, tris, worst)) in census.iter().enumerate() {
        if *tiles_kept == 0 {
            continue;
        }
        info!(
            "terrain LOD: rung δ ≤ {declared:.2} m — {tiles_kept} tiles, {tris} tris, \
             measured worst δ {worst:.4} m, {switch}",
            declared = TERRAIN_LOD_LADDER[rung],
            switch = match view {
                Some(view) => format!(
                    "switch at {:.0} m",
                    rung_switch_distance_m(view, rung, radius)
                ),
                None => "out of range until the view is read".to_string(),
            },
        );
    }
    info!(
        "terrain LOD: {tiles} tiles → {entities} entities, generated in {gen} ms \
         (+{tan} ms tangents), radius {radius:.1} m, {view}",
        tiles = lod.tiles.len(),
        gen = lod.build.as_millis(),
        tan = tangents.as_millis(),
        view = match view {
            Some(view) => format!(
                "view fov {:.3} rad × {:.0} px @ {} px budget",
                view.facts.vfov_rad, view.facts.height_px, view.budget_px
            ),
            None => "no view read yet — every tile at its finest level until the first frame with \
                     facts"
                .to_string(),
        },
    );
    commands.insert_resource(TerrainLodLadder(lod));
}

/// The live ladder, kept so the adaptive layer can rewrite thresholds without regenerating meshes.
/// The `Mesh` in each level has been moved into `Assets<Mesh>` by then — what remains is the
/// census and the geometry of selection.
#[derive(Resource)]
pub(crate) struct TerrainLodLadder(pub(crate) TerrainLod);

/// Sole range writer, consuming `view::track_view_facts`' facts and dead band with this ladder's
/// budget. FOV (optic/magnification) and height (resize/render scale) change at human rates across
/// a few hundred entities. Shared (rung, profile) thresholds add only a handful of permanent
/// range-table slots per visited profile, never per-tile floats. Narrower FOV moves thresholds out;
/// widening it moves them back in.
fn adapt_ranges(
    mut commands: Commands,
    ladder: Option<Res<TerrainLodLadder>>,
    facts: Res<ViewFacts>,
    // The applied PROFILE, not the live one: a ladder spawned before the view is declared carries
    // every tile at its finest level, so the first frame with facts must rewrite even when nothing
    // has moved since.
    mut applied: Local<Option<ViewProfile>>,
    entities: Query<(Entity, &TerrainLodEntity)>,
) {
    // A headless world or a view before terrain spawn has no ladder to adapt.
    let Some(ladder) = ladder else {
        return;
    };
    let wanted = terrain_view(*facts);
    if *applied == Some(wanted) {
        return;
    }
    *applied = Some(wanted);
    for (entity, marker) in &entities {
        let tile = ladder
            .0
            .tiles
            .get(marker.tile)
            .expect("terrain spawn writes every marker's tile index from the installed ladder");
        let range = ladder
            .0
            .ranges(tile, wanted)
            .into_iter()
            .nth(marker.level)
            .expect("terrain spawn writes every marker's level index from its tile's ladder");
        commands.entity(entity).insert(range);
    }
    info!(
        "terrain LOD: view profile → fov {fov:.3} rad × {height:.0} px, thresholds rewritten",
        fov = wanted.facts.vfov_rad,
        height = wanted.facts.height_px,
    );
}

/// Adapt after view facts change in the same frame; `world::spawn_environment` owns generation.
/// Also mounted by the dedicated server's `world::plugin`, which has no window, camera,
/// `view::plugin` or render ladder. Without view facts there is no selection to make.
pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        adapt_ranges
            .run_if(resource_exists::<ViewFacts>)
            // Same frame as the resize that caused it, not the next one.
            .after(crate::view::track_view_facts),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIXTURE view with explicit valid measurements; `ViewFacts` supplies no default.
    fn measured(vfov_rad: f32, height_px: f32) -> ViewFacts {
        ViewFacts::new(vfov_rad, height_px).expect("a measured view")
    }

    /// The reference view the shipped corpus was cut against: the gunner optic at 4K native.
    fn reference_facts() -> ViewFacts {
        measured(
            crate::bake::selected_spec().views[&crate::spec::ViewKind::Gunner]
                .optics
                .vertical_fov(),
            2160.0,
        )
    }

    /// The view cannot leave stale ranges on a marker outside the ladder that spawned it.
    /// FIXTURE DATA: an empty tile separates a missing tile from a missing level, with no meshes.
    fn adapt_invalid_marker(tile: usize, level: usize) {
        let mut app = App::new();
        app.insert_resource(reference_facts())
            .insert_resource(TerrainLodLadder(TerrainLod {
                tiles: vec![TerrainLodTile {
                    min: Vec3::ZERO,
                    max: Vec3::ONE,
                    levels: Vec::new(),
                }],
                bounding_radius_m: 1.0,
                build: std::time::Duration::ZERO,
            }))
            .add_systems(Update, adapt_ranges);
        app.world_mut().spawn(TerrainLodEntity { tile, level });
        app.update();
    }

    #[test]
    #[should_panic(expected = "terrain spawn writes every marker's tile index")]
    fn a_marker_outside_the_spawned_tiles_is_a_bug() {
        adapt_invalid_marker(1, 0);
    }

    #[test]
    #[should_panic(expected = "terrain spawn writes every marker's level index")]
    fn a_marker_outside_its_tiles_levels_is_a_bug() {
        adapt_invalid_marker(0, 0);
    }

    use crate::terrain_grid::tests::shipped_grid;

    /// Terrain normal maps need tangent bases on every published mesh. An OS discovery refusal
    /// must remain an error before any worker runs, never silently select one worker.
    #[test]
    fn tangent_startup_refuses_os_errors_before_writing_meshes() {
        // Fixture: a flat 2x2 grid; its geometry requires a tangent basis but no LOD reduction.
        let grid = HeightGrid::new(vec![0.0; 4].into(), 2, crate::terrain_grid::FIXTURE_EXTENT);
        let mut lod = build(&grid);
        let refusal = generate_tangents(&mut lod, || {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        })
        .expect_err("OS discovery failure must refuse startup");
        assert!(matches!(refusal, TangentStartupError::Parallelism(error)
            if error.kind() == std::io::ErrorKind::PermissionDenied));
        assert!(
            lod.tiles
                .iter()
                .flat_map(|tile| &tile.levels)
                .all(|level| !level.mesh.contains_attribute(Mesh::ATTRIBUTE_TANGENT))
        );
        generate_tangents(&mut lod, || Ok(std::num::NonZeroUsize::new(1).unwrap()))
            .expect("one available fixture worker builds tangent bases");
        assert!(
            lod.tiles
                .iter()
                .flat_map(|tile| &tile.levels)
                .all(|level| level.mesh.contains_attribute(Mesh::ATTRIBUTE_TANGENT))
        );
    }

    /// One tile's height patch, in the layout the hierarchy expects.
    fn patch(grid: &HeightGrid, [ia, ib, ja, jb]: [usize; 4]) -> Vec<f32> {
        let mut heights = Vec::with_capacity((ib - ia + 1) * (jb - ja + 1));
        for j in ja..=jb {
            for i in ia..=ib {
                heights.push(grid.sample(i, j));
            }
        }
        heights
    }

    /// Negative-threshold RTIN must match `terrain_mesh_tiles`' triangle set and winding, not
    /// only its DERIVED two triangles per cell. Alternating-bintree main diagonals can keep the count
    /// and corner heights while changing the surface; this pins `canonicalize_cell_diagonals`.
    #[test]
    fn rtin_at_zero_error_reproduces_the_canonical_tessellation() {
        let full = MESH_TILE_CELLS + 1;
        let hierarchy = rtin::Rtin::new(full);
        // FIXTURE DATA: four full tiles with non-coplanar cell corners expose diagonal changes.
        let size = 2 * MESH_TILE_CELLS + 1;
        let heights = (0..size * size)
            .map(|k| ((k % size) * (k / size) % 7) as f32)
            .collect::<Vec<_>>();
        let grid = HeightGrid::new(
            heights.into(),
            size as u32,
            crate::terrain_grid::FIXTURE_EXTENT,
        );
        let ranges = mesh_tile_node_ranges(&grid);
        let canonical = crate::terrain_grid::terrain_mesh_tiles(&grid);
        assert_eq!(ranges.len(), canonical.len());
        let mut total = 0usize;
        for (t, (range, mesh)) in ranges.into_iter().zip(&canonical).enumerate() {
            let errors = hierarchy.error_pyramid(&patch(&grid, range));
            let mut ours = hierarchy.triangulate(&errors, -1.0);
            assert_eq!(
                ours.len(),
                2 * MESH_TILE_CELLS * MESH_TILE_CELLS,
                "exact extraction must be the full grid"
            );
            total += ours.len();
            let Some(Indices::U32(indices)) = mesh.indices() else {
                panic!("the canonical tiler must carry u32 indices");
            };
            // Both index the tile's own patch row-major, so the triples are directly
            // comparable — once each is rotated to start at its lowest vertex. Rotation preserves
            // winding and is the only freedom a triangle has; emission order is not part of the
            // claim, the tessellation is.
            let normalize = |tri: &mut [u32; 3]| {
                let first = usize::from(tri[1] < tri[0] && tri[1] < tri[2])
                    + 2 * usize::from(tri[2] < tri[0] && tri[2] < tri[1]);
                *tri = [tri[first], tri[(first + 1) % 3], tri[(first + 2) % 3]];
            };
            let mut theirs: Vec<[u32; 3]> = indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|tri| [tri[0], tri[1], tri[2]])
                .collect();
            ours.iter_mut().for_each(&normalize);
            theirs.iter_mut().for_each(&normalize);
            ours.sort_unstable();
            theirs.sort_unstable();
            assert_eq!(
                ours, theirs,
                "tile {t}: the exact extraction is not the canonical anti-diagonal tessellation"
            );
        }
        assert_eq!(total, 2 * (grid.size() as usize - 1).pow(2));
    }

    /// Every kept level's continuous deviation must fit its declared rung, which sets its switch
    /// distance. `worst_deviation_m`'s nodes + centres contain the overlay vertices; node-only
    /// checks can declare 5 cm despite MEASURED 0.323 m error. The separate dense sub-cell test
    /// cross-checks the maximum instead of relying only on this bookkeeping agreement.
    #[test]
    fn every_lod_level_stays_within_its_declared_deviation() {
        let grid = shipped_grid();
        let full = MESH_TILE_CELLS + 1;
        let hierarchy = rtin::Rtin::new(full);
        let lod = build(&grid);
        for (t, (range, tile)) in mesh_tile_node_ranges(&grid)
            .into_iter()
            .zip(&lod.tiles)
            .enumerate()
        {
            let heights = patch(&grid, range);
            let errors = hierarchy.error_pyramid(&heights);
            for level in &tile.levels {
                let declared = TERRAIN_LOD_LADDER[level.rung];
                assert!(
                    level.measured_dev_m <= declared,
                    "tile {t} rung {} measured {} m > declared {declared} m",
                    level.rung,
                    level.measured_dev_m,
                );
                if level.rung == 0 {
                    assert_eq!(level.measured_dev_m, 0.0, "tile {t} rung 0 must be exact");
                    continue;
                }
                let tris = hierarchy.triangulate(&errors, level.extracted_at_m);
                assert_eq!(tris.len(), level.triangles, "tile {t} triangle bookkeeping");
                assert_eq!(
                    worst_deviation_m(&heights, full, &tris),
                    level.measured_dev_m,
                    "tile {t} deviation bookkeeping"
                );
            }
        }
    }

    /// Dense sub-cell probes on the roughest tile must not beat `worst_deviation_m`. Sampling
    /// cannot prove a maximum but can falsify it, exposing a node-only certificate.
    #[test]
    fn the_overlay_maximum_is_not_beaten_by_dense_sub_cell_sampling() {
        let grid = shipped_grid();
        let full = MESH_TILE_CELLS + 1;
        let hierarchy = rtin::Rtin::new(full);
        // The tile with the largest coarse-level deviation — where a missed maximum would show.
        let ranges = mesh_tile_node_ranges(&grid);
        let lod = build(&grid);
        let (worst_tile, _) = lod
            .tiles
            .iter()
            .enumerate()
            .max_by(|a, b| {
                let dev = |tile: &TerrainLodTile| {
                    tile.levels
                        .iter()
                        .map(|level| level.measured_dev_m)
                        .fold(0.0f32, f32::max)
                };
                dev(a.1).total_cmp(&dev(b.1))
            })
            .expect("the map has tiles");
        let heights = patch(&grid, ranges[worst_tile]);
        let errors = hierarchy.error_pyramid(&heights);
        // The canonical surface inside the patch, evaluated the way `height_at` evaluates it.
        let canonical = |x: f64, z: f64| -> f64 {
            let (i0, j0) = ((x as usize).min(full - 2), (z as usize).min(full - 2));
            let (fu, fv) = (x - i0 as f64, z - j0 as f64);
            let h = |i: usize, j: usize| f64::from(heights[j * full + i]);
            let (h00, h10, h01, h11) = (h(i0, j0), h(i0 + 1, j0), h(i0, j0 + 1), h(i0 + 1, j0 + 1));
            if fu + fv <= 1.0 {
                h00 + (h10 - h00) * fu + (h01 - h00) * fv
            } else {
                h11 + (h10 - h11) * (1.0 - fv) + (h01 - h11) * (1.0 - fu)
            }
        };
        for &threshold in &TERRAIN_LOD_LADDER[1..] {
            let tris = hierarchy.triangulate(&errors, threshold);
            let reported = f64::from(worst_deviation_m(&heights, full, &tris));
            // Rasterize each triangle over a 1/8-cell lattice.
            const STEPS: i64 = 8;
            let mut sampled = 0.0f64;
            for tri in &tris {
                let node = |k: u32| (i64::from(k) % full as i64, i64::from(k) / full as i64);
                let ((ax, ay), (bx, by), (cx, cy)) = (node(tri[0]), node(tri[1]), node(tri[2]));
                let (ha, hb, hc) = (
                    f64::from(heights[(ay * full as i64 + ax) as usize]),
                    f64::from(heights[(by * full as i64 + bx) as usize]),
                    f64::from(heights[(cy * full as i64 + cx) as usize]),
                );
                let (ax, ay, bx, by, cx, cy) = (
                    ax * STEPS,
                    ay * STEPS,
                    bx * STEPS,
                    by * STEPS,
                    cx * STEPS,
                    cy * STEPS,
                );
                let area2 = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
                let inv = 1.0 / area2 as f64;
                for q in ay.min(by).min(cy)..=ay.max(by).max(cy) {
                    for p in ax.min(bx).min(cx)..=ax.max(bx).max(cx) {
                        let ea = (cx - bx) * (q - by) - (cy - by) * (p - bx);
                        let eb = (ax - cx) * (q - cy) - (ay - cy) * (p - cx);
                        let ec = (bx - ax) * (q - ay) - (by - ay) * (p - ax);
                        let inside = if area2 > 0 {
                            ea >= 0 && eb >= 0 && ec >= 0
                        } else {
                            ea <= 0 && eb <= 0 && ec <= 0
                        };
                        if !inside {
                            continue;
                        }
                        let plane = (ea as f64 * ha + eb as f64 * hb + ec as f64 * hc) * inv;
                        let truth = canonical(p as f64 / STEPS as f64, q as f64 / STEPS as f64);
                        sampled = sampled.max((plane - truth).abs());
                    }
                }
            }
            assert!(
                sampled <= reported + 1.0e-6,
                "δ {threshold}: dense sampling found {sampled} m, above the reported overlay \
                 maximum {reported} m — the overlay vertex set is incomplete"
            );
        }
    }

    /// RTIN REMOVES vertices, it never moves one: every vertex of every level is an exact grid
    /// sample at its own world position — the pointwise form of ONE-SURFACE.
    #[test]
    fn interior_vertices_are_exact_grid_samples() {
        let grid = shipped_grid();
        let lod = build(&grid);
        let step = grid.world_size() / (grid.size() - 1) as f32;
        let half = grid.half_extent();
        for tile in &lod.tiles {
            for level in &tile.levels {
                let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
                    level.mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                else {
                    panic!("every level must carry f32x3 positions");
                };
                for p in positions {
                    let i = ((p[0] + half) / step).round() as usize;
                    let j = ((p[2] + half) / step).round() as usize;
                    assert_eq!(
                        p[1],
                        grid.sample(i, j),
                        "level vertex {p:?} is not the grid sample at node ({i}, {j})"
                    );
                }
            }
        }
    }

    /// Full-density borders on all levels must meet neighbours vertex-for-vertex: independent
    /// tile selection is crack-free only under this invariant, without skirts.
    #[test]
    fn borders_are_exact_at_every_level() {
        let grid = shipped_grid();
        let full = MESH_TILE_CELLS + 1;
        let hierarchy = rtin::Rtin::new(full);
        let last = (full - 1) as u32;
        for range in mesh_tile_node_ranges(&grid) {
            let errors = hierarchy.error_pyramid(&patch(&grid, range));
            for declared in TERRAIN_LOD_LADDER {
                let tris = hierarchy.triangulate(&errors, declared);
                // Every unit segment of every border must be an edge of some emitted triangle,
                // and no border edge may be longer than one cell.
                let mut seen = vec![[false; 4]; MESH_TILE_CELLS];
                for tri in &tris {
                    for k in 0..3 {
                        let (p, q) = (tri[k], tri[(k + 1) % 3]);
                        let (px, py) = (p % full as u32, p / full as u32);
                        let (qx, qy) = (q % full as u32, q / full as u32);
                        let edge = if py == qy && (py == 0 || py == last) {
                            Some((px.min(qx), px.max(qx), usize::from(py != 0)))
                        } else if px == qx && (px == 0 || px == last) {
                            Some((py.min(qy), py.max(qy), 2 + usize::from(px != 0)))
                        } else {
                            None
                        };
                        if let Some((lo, hi, side)) = edge {
                            assert_eq!(
                                hi - lo,
                                1,
                                "δ {declared} has a border edge {lo}..{hi} — the border row is \
                                 not full density and neighbouring tiles will crack"
                            );
                            seen[lo as usize][side] = true;
                        }
                    }
                }
                for (k, sides) in seen.iter().enumerate() {
                    for (side, present) in sides.iter().enumerate() {
                        assert!(
                            *present,
                            "δ {declared} is missing border segment {k} on side {side}"
                        );
                    }
                }
            }
        }
    }

    /// Ranges partition `[0, ∞)` without double draw (z-fighting/shadow cost) or missing ground.
    /// Check adjacency through the legal FOV interval and a synthetic zero-height profile, which
    /// `ViewFacts::new` refuses. That profile collapses switches to the radius, leaving empty
    /// middle intervals; strict increase is required only for profiles with pixels.
    #[test]
    fn terrain_lod_ranges_are_complementary() {
        let lod = build(&shipped_grid());
        let mut views = vec![
            terrain_view(reference_facts()),
            // Built FIELD-BY-FIELD rather than through `ViewFacts::new`, whose whole job is to
            // refuse this: the chain must survive a degenerate profile even if one ever reaches it.
            ViewProfile {
                facts: ViewFacts {
                    vfov_rad: std::f32::consts::FRAC_PI_4,
                    height_px: 0.0,
                },
                budget_px: TERRAIN_LOD_BUDGET_PX,
            },
        ];
        views.extend(
            [1.0e-4, 0.12, std::f32::consts::FRAC_PI_4, 3.0]
                .map(|fov| terrain_view(measured(fov, 1440.0))),
        );
        for view in views {
            for (t, tile) in lod.tiles.iter().enumerate() {
                let ranges = lod.ranges(tile, view);
                assert_eq!(ranges.len(), tile.levels.len());
                assert_eq!(
                    ranges[0].start_margin.start, 0.0,
                    "tile {t} must be drawn at the camera (view {view:?})"
                );
                assert_eq!(
                    ranges
                        .last()
                        .expect("a tile always has a level")
                        .end_margin
                        .end,
                    f32::INFINITY,
                    "tile {t} must never vanish (view {view:?})"
                );
                for pair in ranges.windows(2) {
                    assert_eq!(
                        pair[0].end_margin.end, pair[1].start_margin.start,
                        "tile {t} has a gap or an overlap in its range chain (view {view:?})"
                    );
                    if view.facts.height_px > 0.0 {
                        assert!(
                            pair[1].start_margin.start > pair[0].start_margin.start,
                            "tile {t} range chain is not strictly increasing (view {view:?})"
                        );
                    }
                    assert!(pair[0].is_abrupt() && pair[1].is_abrupt());
                }
            }
        }
    }

    /// Independently rederive `ranges` thresholds from rung, budget, view and radius; literal metre
    /// thresholds or an unpropagated budget change there must fail. This does not test ECS spawning.
    #[test]
    fn wired_thresholds_are_the_derivation() {
        let lod = build(&shipped_grid());
        let view = terrain_view(reference_facts());
        // Independent re-derivation of the whole chain, from the constants only.
        let radius = lod
            .tiles
            .iter()
            .map(|tile| (tile.max - tile.min).length() * 0.5)
            .fold(0.0f32, f32::max);
        assert_eq!(radius, lod.bounding_radius_m);
        let expected: Vec<f32> = TERRAIN_LOD_LADDER
            .iter()
            .enumerate()
            .map(|(rung, &dev)| {
                if rung == 0 {
                    0.0
                } else {
                    // THE EXACT PROJECTION, written out here rather than called: this test is the
                    // independent side of the derivation, so it must not reach for the same
                    // expression the wiring uses (ADR-0033 §9).
                    dev * view.facts.height_px
                        / (2.0 * (view.facts.vfov_rad / 2.0).tan() * view.budget_px)
                        + radius
                }
            })
            .collect();
        for tile in &lod.tiles {
            for (level, range) in tile.levels.iter().zip(lod.ranges(tile, view)) {
                assert_eq!(
                    range.start_margin.start, expected[level.rung],
                    "rung {} threshold is not its derivation",
                    level.rung
                );
            }
        }
        // And the distinct-value count stays tiny — the range table is permanent and u16-indexed.
        let mut distinct: Vec<u32> = lod
            .tiles
            .iter()
            .flat_map(|tile| lod.ranges(tile, view))
            .map(|range| range.start_margin.start.to_bits())
            .collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert!(
            distinct.len() <= TERRAIN_LOD_LADDER.len(),
            "thresholds are not octave-shared: {} distinct starts",
            distinct.len()
        );
    }

    /// Three kept levels without geometry exercise selection/adaptation without heightmap decode.
    fn synthetic_ladder() -> TerrainLod {
        let level = |rung: usize| TerrainLodLevel {
            rung,
            extracted_at_m: TERRAIN_LOD_LADDER[rung],
            measured_dev_m: TERRAIN_LOD_LADDER[rung],
            triangles: 0,
            mesh: Mesh::new(PrimitiveTopology::TriangleList, TERRAIN_MESH_USAGE),
        };
        let tile = TerrainLodTile {
            min: Vec3::new(-62.5, 0.0, -62.5),
            max: Vec3::new(62.5, 20.0, 62.5),
            levels: vec![level(0), level(2), level(4)],
        };
        TerrainLod {
            bounding_radius_m: tile.bounding_radius_m(),
            tiles: vec![tile],
            build: std::time::Duration::ZERO,
        }
    }

    /// An optic's narrow field magnifies error, so thresholds must move outward; unchanged facts
    /// must leave them there. Assert direction, not just a change, to catch stale startup-view
    /// selection. A few hundred entities and human-rate triggers make adaptation cheap.
    #[test]
    fn the_adaptive_layer_rewrites_thresholds_when_the_view_changes() {
        // Fixture optics: the gunner field is strictly narrower than the commander's.
        let commander_fov = crate::spec::Optics::Unaided(60.0).vertical_fov();
        let optic_fov = commander_fov / 2.0;
        let mut app = App::new();
        // Mount the shared reader: this module consumes view facts, not cameras or windows.
        app.add_plugins((crate::view::plugin, plugin));
        app.insert_resource(TerrainLodLadder(synthetic_ladder()));
        let world = app.world_mut();
        let window = world
            .spawn((Window::default(), bevy::window::PrimaryWindow))
            .id();
        let camera = world
            .spawn((
                Camera3d::default(),
                Projection::Perspective(PerspectiveProjection {
                    fov: commander_fov,
                    ..default()
                }),
                // The declared view the shared reader resolves — the ladder consumes its facts.
                crate::view::PlayerView,
            ))
            .id();
        let entities: Vec<Entity> = (0..3)
            .map(|level| {
                world
                    .spawn((
                        TerrainLodEntity { tile: 0, level },
                        VisibilityRange::abrupt(0.0, f32::INFINITY),
                    ))
                    .id()
            })
            .collect();

        app.update();
        app.world_mut()
            .get_mut::<Window>(window)
            .expect("the test window")
            .resolution
            .set_physical_resolution(2560, 1440);
        app.update();
        let commander: Vec<f32> = entities
            .iter()
            .map(|&entity| {
                app.world()
                    .get::<VisibilityRange>(entity)
                    .expect("the adaptive layer must write a range")
                    .start_margin
                    .start
            })
            .collect();

        // Narrowing the field magnifies the same terrain deviations.
        {
            let mut lens = app
                .world_mut()
                .get_mut::<Projection>(camera)
                .expect("the test camera");
            let Projection::Perspective(projection) = lens.as_mut() else {
                panic!("the test camera must carry a perspective projection");
            };
            projection.fov = optic_fov;
        }
        app.update();
        let optic: Vec<f32> = entities
            .iter()
            .map(|&entity| {
                app.world()
                    .get::<VisibilityRange>(entity)
                    .expect("the adaptive layer must write a range")
                    .start_margin
                    .start
            })
            .collect();

        assert_eq!(commander[0], 0.0, "the exact level always starts at zero");
        assert_eq!(optic[0], 0.0);
        for level in 1..3 {
            assert!(
                optic[level] > commander[level],
                "the optic must push level {level} out ({} vs {})",
                optic[level],
                commander[level],
            );
        }
        // And the rewrite is idempotent: no view change, no work.
        app.update();
        for (level, &entity) in entities.iter().enumerate() {
            assert_eq!(
                app.world()
                    .get::<VisibilityRange>(entity)
                    .expect("range")
                    .start_margin
                    .start,
                optic[level],
            );
        }
    }

    /// Pin [`TERRAIN_MESH_USAGE`]'s render-only contract: losing it silently adds the owner's
    /// DERIVED ~100 MiB permanent CPU copy, which no main-world reader needs.
    #[test]
    fn terrain_meshes_do_not_keep_a_main_world_copy() {
        assert_eq!(
            TERRAIN_MESH_USAGE,
            bevy::asset::RenderAssetUsages::RENDER_WORLD
        );
        let grid = shipped_grid();
        for mesh in crate::terrain_grid::terrain_mesh_tiles(&grid) {
            assert_eq!(mesh.asset_usage, TERRAIN_MESH_USAGE, "level zero");
        }
        for tile in &build(&grid).tiles {
            for level in &tile.levels {
                assert_eq!(
                    level.mesh.asset_usage, TERRAIN_MESH_USAGE,
                    "rung {}",
                    level.rung
                );
            }
        }
    }
}
