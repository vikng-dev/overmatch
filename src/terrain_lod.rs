//! Terrain render LOD: error-bounded triangle ladders for the 64 render tiles the heightmap world
//! already draws (`terrain_grid::terrain_mesh_tiles`), selected declaratively by bevy's
//! [`VisibilityRange`].
//!
//! # What this is, in one paragraph
//!
//! Every render tile is a 129² patch of THE grid (129 = 2⁷ + 1 — [`MESH_TILE_CELLS`] and
//! `GRID_RESOLUTION` already satisfy RTIN's 2ⁿ+1 constraint). For each tile we build the RTIN /
//! Martini right-triangle error pyramid ONCE, then extract one mesh per rung of a declared
//! deviation ladder ([`TERRAIN_LOD_LADDER`]). Triangle count is an OUTPUT, never a tuning dial:
//! the rung says "this mesh is within δ metres of the ground everywhere", and whatever triangle
//! count that costs on this tile is what the tile costs. A flat perimeter tile collapses to a
//! handful of triangles at δ = 2 cm; a ridge tile stays dense until δ = 25 cm.
//!
//! # THE THREE INVARIANTS
//!
//! 1. **ONE-SURFACE (`terrain_grid` module doc) holds by construction.** The levels are generated
//!    at STARTUP from the in-memory [`HeightGrid`] — the identical object the oracle, the Avian
//!    collider and spawn placement read. There is no build tool, no baked artefact and therefore
//!    no way for a stale product to diverge from the decoded grid. (Re-arm a versioned manifest
//!    the day generation becomes a build step for streaming or a 5 km map.)
//!
//!    RTIN only ever REMOVES vertices, never moves one, so every kept vertex is an exact grid
//!    sample — `interior_vertices_are_exact_grid_samples` pins that pointwise. **That is a claim
//!    about vertices and NOT about the surface**, and treating it as the latter is the mistake this
//!    module was shipped with once already: RTIN also changes cell CONNECTIVITY, splitting cells
//!    along the main diagonal where the canonical surface uses the anti-diagonal. Two tessellations
//!    can agree at every grid node and still be different ground between them. So:
//!    [`rtin::Rtin::canonicalize_cell_diagonals`] re-splits every fully refined cell onto the
//!    canonical diagonal, and certification measures the exact CONTINUOUS maximum over the overlay
//!    of the two triangulations ([`worst_deviation_m`]) rather than sampling nodes.
//!
//! 2. **NO SKIRTS. Every level keeps the exact full-density border row on all four edges.**
//!    Borders are therefore identical across every level of every tile, so any level of tile A
//!    meets any level of tile B vertex-for-vertex: crack-free BY CONSTRUCTION, with no neighbour
//!    state, no 16 edge-index variants, and no metre-class curtain walls casting grid-aligned
//!    shadows under the 17° sun. This is enforced inside the error pyramid (see
//!    [`rtin::Rtin::error_pyramid`]), not by post-processing, which keeps RTIN's restricted-bintree
//!    property intact and so leaves the tile INTERIOR crack-free too.
//!
//! 3. **Thresholds are octave-quantized and SHARED.** A tile-level's MEASURED deviation is
//!    assigned UP to the smallest ladder rung that contains it, and the switch distance is a pure
//!    function of (rung, view profile). Bevy retains a permanent range-table slot per distinct
//!    [`VisibilityRange`] value for the lifetime of the app and indexes them with a `u16`
//!    (`bevy_render-0.19.0/src/view/visibility/range.rs`), so per-tile floats are a slow leak that
//!    ends in slot-zero aliasing. With this shape the table sees at most a couple of dozen values
//!    no matter how many tiles exist.
//!
//! # The landmine this module is built around
//!
//! `check_visibility_ranges` (`bevy_camera-0.19.0/src/visibility/range.rs:263`) measures the
//! camera distance to `entity_transform.translation()` — the WORLD ORIGIN for every terrain tile,
//! since tiles carry world-space vertices at `Transform::IDENTITY` — UNLESS `use_aabb` is set, in
//! which case it measures to `affine.transform_point3a(aabb.center)`. Note carefully: the AABB
//! **CENTRE**, not its nearest point. So `use_aabb: true` + an explicit [`Aabb`] gives us the tile
//! centre as the anchor (which is what we want, and identical for every level of a tile because we
//! pin the same exact-level bounds on all of them), and the switch distances must then add the
//! tile bounding radius themselves to stay conservative about the tile's NEAREST surface.
//!
//! Because `use_aabb` resolves the anchor through the pinned AABB, tile-LOCAL vertices buy nothing
//! here: an identity transform applied to a pinned world-space AABB gives exactly the same anchor
//! a tile-local mesh at a tile-centre transform would. So the vertices stay WORLD-SPACE, which
//! keeps `terrain_mesh_tiles` and every ONE-SURFACE assertion made against it (`mesh vertex is the
//! grid sample at node (i, j)`) untouched.
//!
//! `NoAutoAabb` is not optional either: bevy's `calculate_bounds` re-derives the AABB on
//! `Changed<Mesh3d>` (`bevy_camera-0.19.0/src/visibility/mod.rs:568`), which fires on spawn and
//! would replace our pinned bounds with each LEVEL's own — drifting the anchor per level and
//! opening sub-metre gaps and overlaps in the range chain.

use std::time::Instant;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::{NoAutoAabb, VisibilityRange};
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;

use crate::terrain_grid::{
    HeightGrid, MESH_TILE_CELLS, TEXTURE_TILE_M, mesh_tile_node_ranges, node_world_coord,
    surface_normal_at,
};

/// The DECLARED deviation ladder, metres. A level built at rung `r` is guaranteed to lie within
/// `TERRAIN_LOD_LADDER[r]` of the grid surface at every grid node inside the tile.
///
/// Rung 0 is the exact surface (every grid sample kept) and is generated by the existing
/// [`crate::terrain_grid::terrain_mesh_tiles`] path, so the shipped close-up picture is exactly the
/// picture that shipped before this module existed.
///
/// The rungs are a shared error VOCABULARY, not a density schedule (codex finding 9): a tile only
/// carries the rungs that actually produce a distinct mesh on it. The spacing is ~octave from
/// 2 cm up, which is where this terrain's own error curve lives — measured whole-map RTIN counts
/// are 776 k tris at 2 cm, 490 k at 5 cm, 292 k at 10 cm and 42 k at 50 cm.
pub(crate) const TERRAIN_LOD_LADDER: [f32; 6] = [0.0, 0.02, 0.05, 0.10, 0.25, 0.50];

/// Screen-space error budget, PIXELS: the on-screen size a level's deviation is allowed to project
/// to before the next-finer level must take over.
///
/// THE SEAM: when `settings` grows a player-facing LOD-quality row this becomes its value. Until
/// then it is one constant with one home, and the whole selection chain derives from it — the
/// derivation test re-computes every wired threshold from this number and fails if a literal ever
/// creeps into the wiring.
pub(crate) const TERRAIN_LOD_BUDGET_PX: f32 = 1.0;

/// The screen-space-error selection rule: the distance (metres) at which a world-space deviation
/// of `dev_m` projects to exactly `budget_px` pixels in a view of vertical field `fov_y_rad`
/// rendered `height_px` pixels tall.
///
/// `D = dev_m · height_px / (fov_y_rad · budget_px)`
///
/// SMALL-ANGLE, DELIBERATELY. The exact form divides by `2·tan(fov/2)` rather than `fov`, and
/// `2·tan(fov/2) ≥ fov` always — so this form returns a LARGER distance, i.e. it holds the finer
/// level closer to the camera than strictly necessary. Measured on our two views: 0.1 % in the
/// gunner optic (fov 0.12 rad) and 5.5 % conservative in the commander view (fov π/4). That is an
/// error in the safe direction and it keeps one closed-form expression across every view; do not
/// "fix" it into a `tan` call without re-deriving the wired thresholds.
pub(crate) fn sub_pixel_distance_m(
    dev_m: f32,
    height_px: f32,
    fov_y_rad: f32,
    budget_px: f32,
) -> f32 {
    if dev_m <= 0.0 {
        return 0.0;
    }
    dev_m * height_px / (fov_y_rad * budget_px)
}

/// The view the terrain ladder is currently selected FOR: one perspective camera's vertical field
/// and the pixel height it actually renders at (window physical height × `render_scale`).
///
/// There is exactly one 3-D camera in the game (`camera::spawn_camera`) and the gunner optic
/// swaps its `Projection` fov in place, so "the view profile" is a single live pair — which is
/// what makes the adaptive layer affordable here and not on the shoe.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct TerrainLodView {
    /// Vertical field of view, radians.
    pub(crate) fov_y_rad: f32,
    /// Rendered height of the main pass, pixels.
    pub(crate) height_px: f32,
    /// Screen-space error budget, pixels.
    pub(crate) budget_px: f32,
}

impl Default for TerrainLodView {
    fn default() -> Self {
        // The narrow optic and a modest window: the conservative pre-window guess, replaced by the
        // adaptive layer on the first frame that has a real window.
        Self {
            fov_y_rad: crate::camera::GUNNER_FOV_FALLBACK,
            height_px: 1080.0,
            budget_px: TERRAIN_LOD_BUDGET_PX,
        }
    }
}

impl TerrainLodView {
    /// A live profile from a camera's field and a rendered pixel height.
    ///
    /// A NON-POSITIVE height is not a small window, it is an ABSENT one — a window bevy has not
    /// sized yet at Startup, or a zero render scale. Taking it literally would collapse every
    /// switch distance onto the bounding radius and put the COARSEST level 94 m from the camera on
    /// the frames the player first sees, which is the exact opposite of the conservative direction.
    /// So an absent height falls back to the default profile's, and the next frame with a real
    /// window corrects it.
    pub(crate) fn new(fov_y_rad: f32, height_px: f32) -> Self {
        let default = Self::default();
        Self {
            fov_y_rad,
            height_px: if height_px > 0.0 {
                height_px
            } else {
                default.height_px
            },
            ..default
        }
    }

    /// The distance (metres, camera → tile CENTRE) at or beyond which rung `rung` is legal for a
    /// tile whose bounding radius is `radius_m`.
    ///
    /// Two terms, and both are load-bearing:
    /// * `sub_pixel_distance_m` — when the rung's deviation stops being worth a pixel;
    /// * `+ radius_m` — because `use_aabb` measures to the tile's CENTRE (verified in the vendored
    ///   source, see the module doc) while the deviation lives on the tile's nearest SURFACE,
    ///   which can be a full bounding radius closer to the camera. One shared radius (the largest
    ///   over all tiles) rather than a per-tile float, so the range table stays small.
    ///
    /// Rung 0 is the exact surface: it starts at the camera, always.
    pub(crate) fn switch_distance_m(self, rung: usize, radius_m: f32) -> f32 {
        if rung == 0 {
            return 0.0;
        }
        sub_pixel_distance_m(
            TERRAIN_LOD_LADDER[rung],
            self.height_px,
            self.fov_y_rad,
            self.budget_px,
        ) + radius_m
    }
}

/// One extracted level of one tile.
pub(crate) struct TerrainLodLevel {
    /// Index into [`TERRAIN_LOD_LADDER`] this level's MEASURED deviation was quantized up to. THIS
    /// is what the switch distance is computed from, and `measured_dev_m ≤ TERRAIN_LOD_LADDER[rung]`
    /// always holds.
    pub(crate) rung: usize,
    /// The threshold fed to the RTIN extraction that produced this mesh.
    ///
    /// NOT the guarantee, and deliberately recorded separately from [`Self::rung`]: MEASURED on the
    /// shipped map, the pyramid under-reports the true continuous deviation by up to 15.9×. That is
    /// not a port bug, it is what the metric means, twice over. The pyramid records at each node the
    /// error of dropping that node from ITS OWN level's interpolation, so per-level errors COMPOUND
    /// down the hierarchy rather than dominating; and it is a NODE metric, blind to everything the
    /// two tessellations do between nodes. Martini/Cesium ship it as a heuristic and so do we — but
    /// we never declare it. The declared rung comes from [`Self::measured_dev_m`], the exact
    /// continuous maximum.
    pub(crate) extracted_at_m: f32,
    /// The EXACT continuous maximum of |this level − the canonical surface| over the whole tile,
    /// metres ([`worst_deviation_m`]) — every point, not every node. Always ≤ the declared rung;
    /// that is the invariant the switch distance rests on.
    pub(crate) measured_dev_m: f32,
    /// Triangles in the extracted mesh (an OUTPUT of the error bound, never a target).
    pub(crate) triangles: usize,
    /// The renderable mesh: world-space positions, full-grid central-difference normals, world-XZ
    /// UVs and (after spawn) a mikktspace tangent basis.
    pub(crate) mesh: Mesh,
}

/// One render tile's ladder.
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

/// The whole map's ladder plus the numbers the census line reports.
pub(crate) struct TerrainLod {
    /// One entry per render tile, in `mesh_tile_node_ranges` order.
    pub(crate) tiles: Vec<TerrainLodTile>,
    /// The ONE shared bounding radius every switch distance adds — the largest tile radius on the
    /// map, so the threshold is conservative for every tile without a per-tile float.
    pub(crate) bounding_radius_m: f32,
    /// Wall time of the whole generation, for the startup census.
    pub(crate) build: std::time::Duration,
}

impl TerrainLod {
    /// The complementary visibility ranges for one tile: `[0, s₁) [s₁, s₂) … [sₙ, ∞)`, every
    /// distance covered exactly once, so a tile is never drawn twice and never vanishes.
    pub(crate) fn ranges(
        &self,
        tile: &TerrainLodTile,
        view: TerrainLodView,
    ) -> Vec<VisibilityRange> {
        let starts: Vec<f32> = tile
            .levels
            .iter()
            .map(|level| view.switch_distance_m(level.rung, self.bounding_radius_m))
            .collect();
        (0..starts.len())
            .map(|k| {
                let start = starts[k];
                let end = starts.get(k + 1).copied().unwrap_or(f32::INFINITY);
                VisibilityRange {
                    start_margin: start..start,
                    end_margin: end..end,
                    // See the module doc: without this the distance is measured to the world
                    // ORIGIN for every tile, because terrain vertices are world-space at identity.
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

/// Build the whole map's LOD ladder from the decoded grid.
///
/// Rung 0 is taken from [`crate::terrain_grid::terrain_mesh_tiles`] — the shipped full-density
/// path, untouched — so the close-up picture is exactly what it was; rungs 1.. come from RTIN.
/// `rtin_at_zero_error_reproduces_the_full_grid` pins the two paths to the same triangle count, so
/// this is one surface described two ways, not two surfaces.
///
/// A grid whose tiles are not 2ⁿ+1 (test fixtures, a re-authored map at another resolution) simply
/// gets a one-level ladder: RTIN's hierarchy does not exist there, and a silently-wrong
/// triangulation would be far worse than no LOD.
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
                // Drop DOMINATED levels: a threshold that buys no triangles on this tile (a flat
                // perimeter tile is already minimal at 2 cm) would only cost a range-table slot
                // and a draw call's worth of bookkeeping.
                if previous.as_ref().is_some_and(|p| *p == tris) {
                    continue;
                }
                previous = Some(tris.clone());
                let measured = worst_deviation_m(&heights, full, &tris);
                // THE DECLARED RUNG IS THE MEASUREMENT, NOT THE THRESHOLD. The extraction
                // threshold is the pyramid's heuristic (see `TerrainLodLevel::extracted_at_m` —
                // measured to under-report by up to ~1.6× on this map), so the rung is the
                // MEASURED deviation quantized UP to the smallest ladder value that contains it.
                // A mesh extracted at 5 cm that really deviates 7.7 cm declares 10 cm and switches
                // in at the 10 cm distance; a mesh extracted at 50 cm that really deviates 3 cm
                // declares 5 cm and switches in nearer than its threshold would suggest. Both
                // directions happen, and both are safe only because this reads the measurement.
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

/// A tile-local RTIN triangulation → a renderable mesh.
///
/// Positions are the grid's own samples in WORLD space (unchanged from the shipped layout);
/// normals come from [`surface_normal_at`] — the FULL-GRID central difference at the kept node,
/// never a normal recomputed from the simplified triangles, so shading does not pop when geometry
/// does; UVs are the same world-XZ tiling every level and the collider-matched surface share.
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
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// The EXACT continuous worst deviation between a triangulation and THE canonical surface over the
/// tile, metres — not a sampled statistic.
///
/// # Why node sampling was wrong, and why this is exact
///
/// The first version of this function evaluated only grid NODES, on the reasoning that RTIN removes
/// vertices without moving them. That reasoning is false as a claim about the SURFACE: RTIN also
/// changes cell CONNECTIVITY. The canonical surface splits every cell along the ANTI-diagonal
/// (`HeightGrid::height_at`, and parry's own triangulation with it); RTIN's alternating bintree
/// gives roughly half its leaf cells the MAIN diagonal instead. Two triangulations of the same four
/// corner heights, agreeing at all four nodes and disagreeing by half the cell's twist
/// `|h₀₀ + h₁₁ − h₁₀ − h₀₁| / 2` at the centre. Codex MEASURED a case at 0.323 m — twelve times the
/// node statistic — on a level declared at 5 cm.
///
/// Both surfaces are piecewise linear over the same square, so their difference is piecewise linear
/// over the OVERLAY (common refinement) of the two triangulations, and its maximum is attained at
/// an overlay VERTEX. Enumerating those vertices is not a search:
///
/// * every vertex of either triangulation is a grid node;
/// * LOD edges are axis-aligned along grid lines, or 45° through grid nodes — so within any cell a
///   LOD edge is a full cell diagonal, never a partial chord;
/// * a LOD axis edge lies on a grid line, and a canonical anti-diagonal touches grid lines only at
///   its two endpoints, which are nodes — no new vertex;
/// * a LOD 45° edge crosses grid lines only at nodes — no new vertex;
/// * within a cell, a LOD diagonal either COINCIDES with the canonical anti-diagonal (no new
///   vertex) or is the MAIN diagonal, which meets it at exactly one point: the cell CENTRE.
///
/// So the overlay vertex set is exactly `grid nodes ∪ centres of cells the LOD splits the other
/// way`, and evaluating at every node AND every cell centre is a superset of it. Centres inside a
/// single linear region are redundant, never wrong. Hence: EXACT, in one pass, no sampling
/// parameter to tune and nothing to alias past.
///
/// Implementation works in DOUBLED node coordinates, where a node is `(2i, 2j)` and a cell centre is
/// `(2i+1, 2j+1)` — so both live on one integer lattice (the same-parity points) and every
/// inside-test stays exact integer arithmetic. The canonical height at a cell centre has a closed
/// form: the centre lies ON the anti-diagonal, where `height_at` reduces to the mean of the two
/// corners that diagonal joins.
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
                // Edge functions against each edge, all sharing the sign of `area2` inside the
                // triangle (zero on an edge counts — a shared edge is covered twice and both
                // cover-events agree, which is exactly what "the surface here" means).
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

        /// The error pyramid: for every non-corner node, the largest vertical error that removing
        /// it (and everything below it) would introduce, in the same units as `heights`.
        ///
        /// Bottom-up and MONOTONE — a parent's entry is the max of its own midpoint error and both
        /// children's entries. That monotonicity is the whole reason the extraction is crack-free
        /// inside the tile: if a triangle splits, the triangle sharing its hypotenuse splits too,
        /// because they read the same entry.
        ///
        /// # THE BORDER RULE (no skirts)
        ///
        /// Every non-corner node on the tile's four edges is seeded to `+∞` BEFORE the fold. Each
        /// non-corner node is the hypotenuse midpoint of exactly one diamond (or of one unpaired
        /// border triangle), so an infinite entry forces that triangle to split at every threshold,
        /// and the fold carries the infinity up the ancestor chain — which force-splits exactly the
        /// graded band of triangles that reach the border and nothing else. The result is the exact
        /// full-density border row on all four edges of every level, with the restricted-bintree
        /// property still intact (we changed the pyramid, not the extraction), so the tile interior
        /// stays crack-free too.
        ///
        /// Seeding before the fold (rather than post-processing the extraction) is what makes this
        /// safe: forcing splits after the fact would break the neighbour agreement the monotone
        /// pyramid guarantees and open T-junctions in the tile INTERIOR.
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

        /// Extract the triangulation whose deviation is bounded by `max_error`, as node indices
        /// (`v * size + u`) in the tile's own patch.
        ///
        /// `max_error < 0` extracts the EXACT surface (every error is ≥ 0, so every splittable
        /// triangle splits): `2 · cells²` triangles, and — after the diagonal canonicalization
        /// below — the identical triangle SET `terrain_mesh_tiles` builds. That equality is the
        /// sanity gate, and it is a connectivity claim, not a count.
        /// `max_error == 0` is NOT the same thing — a perfectly flat region has error exactly zero
        /// and would collapse.
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

        /// Re-split every FULLY REFINED cell along the canonical ANTI-diagonal.
        ///
        /// # Why this exists (codex 2026-08-02, finding 1)
        ///
        /// The canonical surface splits every cell along the anti-diagonal — `HeightGrid::height_at`
        /// evaluates it and parry's heightfield triangulates it, and the ONE-SURFACE invariant is
        /// that claim. RTIN's bintree does NOT: its leaf orientation is fixed by position parity,
        /// so roughly half of its leaf cells come out on the MAIN diagonal. The two agree at all
        /// four corners and disagree by half the cell's twist, `|h₀₀ + h₁₁ − h₁₀ − h₀₁| / 2`, at the
        /// centre — invisible to node sampling, MEASURED at 0.323 m in one shipped cell.
        ///
        /// Constraining the hierarchy itself to the anti-diagonal is not possible: the alternating
        /// bintree's leaf parity is structural, not a choice at extraction time. But the fix does
        /// not need the hierarchy — a cell covered by exactly TWO leaf triangles can be re-split
        /// locally, because both splittings have the IDENTICAL four boundary edges and the
        /// identical vertex set. Only the interior diagonal changes, so nothing outside the cell can
        /// observe it: no crack, no T-junction, no change in triangle count.
        ///
        /// A cell covered by one leaf and one flank of a coarser triangle is left alone (it cannot
        /// be re-split without touching that coarser triangle). Those cells keep a genuine
        /// simplification error, which is exactly what `worst_deviation_m`'s overlay maximum now
        /// measures and what the declared rung is quantized from.
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
                // Same four corners, same four boundary edges, canonical interior diagonal — and
                // in the `(a, b, c)` shape whose winding fix reproduces `terrain_mesh_tiles`'
                // `[i00, i01, i10]` / `[i10, i01, i11]` exactly.
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

/// Give every level its mikktspace tangent basis, across the machine's cores.
///
/// Required, no fallback, at EVERY level and not just the finest (ADR-0011: a level that cannot
/// carry a tangent basis is a broken ship, not a degraded one — the normal map has no frame to
/// rotate its directions into and the ground renders flat and greasy).
///
/// Parallel because it is the startup cost that matters: MEASURED at 3.19 s single-threaded over
/// the shipped map's 3.83 M pyramid triangles, against 134 ms for the whole of generation. The
/// generator is per-mesh and stateless, so splitting the level list across threads changes nothing
/// about the result — the same tangents in the same order, just sooner.
fn generate_tangents(lod: &mut TerrainLod) {
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
        return;
    }
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(pending.len());
    let chunk = pending.len().div_ceil(threads);
    std::thread::scope(|scope| {
        for batch in pending.chunks_mut(chunk) {
            scope.spawn(move || {
                for (tile, level, mesh) in batch {
                    mesh.generate_tangents().unwrap_or_else(|err| {
                        panic!(
                            "terrain LOD tile {tile} level {level} failed mikktspace tangent \
                             generation: {err}"
                        )
                    });
                }
            });
        }
    });
}

/// Build and spawn the whole terrain LOD ladder, logging the per-rung census. Called from
/// `world::spawn_environment` in place of the flat tile loop, on windowed compositions only.
pub(crate) fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: &Handle<StandardMaterial>,
    grid: &HeightGrid,
    view: TerrainLodView,
) {
    let mut lod = build(grid);
    let tangents_started = Instant::now();
    generate_tangents(&mut lod);
    let tangents = tangents_started.elapsed();
    let mut census = vec![(0usize, 0usize, 0.0f32); TERRAIN_LOD_LADDER.len()];
    let mut entities = 0usize;
    let radius = lod.bounding_radius_m;
    for (tile_index, tile) in lod.tiles.iter_mut().enumerate() {
        let aabb = Aabb::from_min_max(tile.min, tile.max);
        let starts: Vec<f32> = tile
            .levels
            .iter()
            .map(|level| view.switch_distance_m(level.rung, radius))
            .collect();
        for (level_index, level) in tile.levels.iter_mut().enumerate() {
            let (tiles_kept, tris, worst) = &mut census[level.rung];
            *tiles_kept += 1;
            *tris += level.triangles;
            *worst = worst.max(level.measured_dev_m);
            // The mesh (already tangented above) is MOVED out of the ladder here — what stays
            // behind is the census, not the geometry, because the whole pyramid resident twice is
            // ~200 MB of nothing.
            let mesh = std::mem::replace(
                &mut level.mesh,
                Mesh::new(
                    PrimitiveTopology::TriangleList,
                    RenderAssetUsages::default(),
                ),
            );
            let start = starts[level_index];
            let end = starts
                .get(level_index + 1)
                .copied()
                .unwrap_or(f32::INFINITY);
            commands.spawn((
                Transform::IDENTITY,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material.clone()),
                // The pinned anchor: the SAME bounds on every level of the tile, and no auto-AABB
                // to overwrite them on the spawn-frame `Changed<Mesh3d>` (module doc).
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
             measured worst δ {worst:.4} m, switch at {switch:.0} m",
            declared = TERRAIN_LOD_LADDER[rung],
            switch = view.switch_distance_m(rung, radius),
        );
    }
    info!(
        "terrain LOD: {tiles} tiles → {entities} entities, generated in {gen} ms \
         (+{tan} ms tangents), radius {radius:.1} m, view fov {fov:.3} rad × {height:.0} px \
         @ {budget} px budget",
        tiles = lod.tiles.len(),
        gen = lod.build.as_millis(),
        tan = tangents.as_millis(),
        fov = view.fov_y_rad,
        height = view.height_px,
        budget = view.budget_px,
    );
    commands.insert_resource(TerrainLodLadder(lod));
    commands.insert_resource(view);
}

/// The live ladder, kept so the adaptive layer can rewrite thresholds without regenerating meshes.
/// The `Mesh` in each level has been moved into `Assets<Mesh>` by then — what remains is the
/// census and the geometry of selection.
#[derive(Resource)]
pub(crate) struct TerrainLodLadder(pub(crate) TerrainLod);

/// Relative field-of-view change that must accumulate before the thresholds are rewritten.
///
/// The optic toggle is a 6.5× jump (π/4 → 0.12 rad), so this never gates a real view change; what
/// it gates is a magnification slider being dragged, where a rewrite per frame would be a per-frame
/// walk of a few hundred entities for a sub-pixel difference.
const FOV_HYSTERESIS: f32 = 0.10;

/// Rewrite every terrain LOD threshold when the view profile changes — fov (the optic toggle, a
/// magnification step), rendered height (window resize), or render scale.
///
/// Affordable BECAUSE terrain is a few hundred entities and the profile changes at human rate.
/// The thresholds stay octave-shared: this writes at most one distinct value per (rung, profile),
/// never a per-tile float, so the permanent range table grows by a handful of slots per profile
/// the player actually visits.
fn adapt_ranges(
    mut commands: Commands,
    ladder: Option<Res<TerrainLodLadder>>,
    current: Option<ResMut<TerrainLodView>>,
    camera: Query<&Projection, With<Camera3d>>,
    windows: Query<&Window>,
    scale: Option<Res<crate::render_scale::RenderScale>>,
    entities: Query<(Entity, &TerrainLodEntity)>,
) {
    let (Some(ladder), Some(mut current)) = (ladder, current) else {
        return;
    };
    let Ok(Projection::Perspective(projection)) = camera.single() else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    let wanted = TerrainLodView::new(
        projection.fov,
        window.physical_height() as f32 * scale.map_or(1.0, |scale| scale.0),
    );
    let fov_moved = (wanted.fov_y_rad - current.fov_y_rad).abs()
        > FOV_HYSTERESIS * current.fov_y_rad.max(f32::MIN_POSITIVE);
    if !fov_moved && wanted.height_px == current.height_px && wanted.budget_px == current.budget_px
    {
        return;
    }
    *current = wanted;
    for (entity, marker) in &entities {
        let Some(tile) = ladder.0.tiles.get(marker.tile) else {
            continue;
        };
        if let Some(range) = ladder.0.ranges(tile, wanted).get(marker.level) {
            commands.entity(entity).insert(range.clone());
        }
    }
    info!(
        "terrain LOD: view profile → fov {fov:.3} rad × {height:.0} px, thresholds rewritten",
        fov = wanted.fov_y_rad,
        height = wanted.height_px,
    );
}

/// Mount the adaptive layer. Generation itself is driven by `world::spawn_environment`.
pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Update, adapt_ranges);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain_grid::{GRID_RESOLUTION, tests::shipped_grid};

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

    /// THE sanity gate (memo §2), strengthened from a COUNT to a CONNECTIVITY claim: the extraction
    /// at a negative threshold must reproduce not merely `2 · 128²` triangles per tile, but the
    /// IDENTICAL triangle set `terrain_mesh_tiles` builds — same triples, same winding.
    ///
    /// Counting alone was the hole codex found: RTIN's alternating bintree emits the right NUMBER
    /// of leaf triangles while splitting roughly half its cells along the main diagonal instead of
    /// parry's anti-diagonal, which is a different SURFACE with identical corner heights. A count
    /// test cannot see that; a set comparison against the canonical tessellation can, and it is
    /// what pins `canonicalize_cell_diagonals` to the ONE-SURFACE invariant.
    #[test]
    fn rtin_at_zero_error_reproduces_the_canonical_tessellation() {
        let full = MESH_TILE_CELLS + 1;
        let hierarchy = rtin::Rtin::new(full);
        let grid = shipped_grid();
        assert_eq!(grid.size(), GRID_RESOLUTION);
        let ranges = mesh_tile_node_ranges(&grid);
        assert_eq!(ranges.len(), 64, "the shipped map is 8×8 render tiles");
        let canonical = crate::terrain_grid::terrain_mesh_tiles(&grid);
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
            // Both index the tile's own 129² patch row-major, so the triples are directly
            // comparable — once each is rotated to start at its lowest vertex. Rotation preserves
            // winding and is the only freedom a triangle has; emission order is not part of the
            // claim, the tessellation is.
            let normalize = |tri: &mut [u32; 3]| {
                let first = usize::from(tri[1] < tri[0] && tri[1] < tri[2])
                    + 2 * usize::from(tri[2] < tri[0] && tri[2] < tri[1]);
                *tri = [tri[first], tri[(first + 1) % 3], tri[(first + 2) % 3]];
            };
            let mut theirs: Vec<[u32; 3]> = indices
                .chunks_exact(3)
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
        assert_eq!(total, 2 * 1024 * 1024, "2,097,152 triangles over the map");
    }

    /// LEVEL ZERO IS UNCHANGED, asserted against the constants rather than against itself.
    ///
    /// `terrain_grid` grew three shared helpers for the ladder (`node_world_coord`,
    /// `surface_normal_at`, `mesh_tile_node_ranges`) and `terrain_mesh_tiles` was rewritten in terms
    /// of them. That is a refactor of the ONE surface every consumer reads, so it gets the regression
    /// codex asked for: every vertex, normal, UV and index of every shipped tile recomputed here
    /// from the raw constants — the pre-refactor expressions, no helper called — plus a content hash
    /// over the whole set as the cheap tripwire for future diffs.
    #[test]
    fn level_zero_is_the_pre_lod_tiler_bit_for_bit() {
        use crate::terrain_grid::{TEXTURE_TILE_M, WORLD_HALF_EXTENT, WORLD_SIZE};
        let grid = shipped_grid();
        let n = grid.size() as usize;
        let cells = n - 1;
        let step = WORLD_SIZE / cells as f32;
        let world_at = |k: usize| -WORLD_HALF_EXTENT + k as f32 * step;
        let normal_at = |i: usize, j: usize| -> [f32; 3] {
            let (il, ih) = (i.saturating_sub(1), (i + 1).min(n - 1));
            let (jl, jh) = (j.saturating_sub(1), (j + 1).min(n - 1));
            let dhdx = (grid.sample(ih, j) - grid.sample(il, j)) / ((ih - il) as f32 * step);
            let dhdz = (grid.sample(i, jh) - grid.sample(i, jl)) / ((jh - jl) as f32 * step);
            Vec3::new(-dhdx, 1.0, -dhdz).normalize().to_array()
        };
        // FNV-1a over the exact bit patterns, so a one-ulp drift anywhere fails.
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |bits: u32| {
            hash = (hash ^ u64::from(bits)).wrapping_mul(0x0000_0100_0000_01b3);
        };
        let tiles = crate::terrain_grid::terrain_mesh_tiles(&grid);
        assert_eq!(tiles.len(), 64);
        let tiles_per_side = cells.div_ceil(MESH_TILE_CELLS);
        let mut tile = 0usize;
        for tz in 0..tiles_per_side {
            for tx in 0..tiles_per_side {
                let ia = tx * MESH_TILE_CELLS;
                let ib = (ia + MESH_TILE_CELLS).min(cells);
                let ja = tz * MESH_TILE_CELLS;
                let jb = (ja + MESH_TILE_CELLS).min(cells);
                let (w, h) = (ib - ia + 1, jb - ja + 1);
                let mesh = &tiles[tile];
                let (
                    Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)),
                    Some(bevy::mesh::VertexAttributeValues::Float32x3(normals)),
                    Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)),
                    Some(Indices::U32(indices)),
                ) = (
                    mesh.attribute(Mesh::ATTRIBUTE_POSITION),
                    mesh.attribute(Mesh::ATTRIBUTE_NORMAL),
                    mesh.attribute(Mesh::ATTRIBUTE_UV_0),
                    mesh.indices(),
                )
                else {
                    panic!("tile {tile} lost an attribute");
                };
                assert_eq!(
                    (positions.len(), normals.len(), uvs.len()),
                    (w * h, w * h, w * h)
                );
                for j in ja..=jb {
                    let z = world_at(j);
                    for i in ia..=ib {
                        let x = world_at(i);
                        let k = (j - ja) * w + (i - ia);
                        assert_eq!(
                            positions[k],
                            [x, grid.sample(i, j), z],
                            "tile {tile} vertex"
                        );
                        assert_eq!(normals[k], normal_at(i, j), "tile {tile} normal");
                        assert_eq!(
                            uvs[k],
                            [x / TEXTURE_TILE_M, z / TEXTURE_TILE_M],
                            "tile {tile} uv"
                        );
                        for value in positions[k].iter().chain(&normals[k]) {
                            eat(value.to_bits());
                        }
                        for value in &uvs[k] {
                            eat(value.to_bits());
                        }
                    }
                }
                let mut expected: Vec<u32> = Vec::with_capacity((w - 1) * (h - 1) * 6);
                for j in 0..h - 1 {
                    for i in 0..w - 1 {
                        let i00 = (j * w + i) as u32;
                        let (i10, i01) = (i00 + 1, i00 + w as u32);
                        expected.extend_from_slice(&[i00, i01, i10, i10, i01, i01 + 1]);
                    }
                }
                assert_eq!(indices, &expected, "tile {tile} indices");
                for &index in indices {
                    eat(index);
                }
                tile += 1;
            }
        }
        // The tripwire value, produced by this test on the shipped map. It is a CONSEQUENCE of the
        // assertions above, not an independent claim — if it moves, one of them moved first.
        assert_eq!(
            hash, 0x70e7_f9e5_e6cf_c91b,
            "the level-zero surface changed; the assertions above name which part"
        );
    }

    /// The POSITIONAL certification (brief §6a): every kept level, on every tile of the shipped map,
    /// must lie within its DECLARED rung of the CONTINUOUS canonical surface — not merely at grid
    /// nodes. The declared rung is what the switch distance is computed from, so a level that
    /// quietly exceeds it anywhere is a level shown too close.
    ///
    /// Certified against the OVERLAY maximum (see `worst_deviation_m`): grid nodes AND cell centres,
    /// which together contain every vertex of the common refinement of the two triangulations, so
    /// the maximum of their piecewise-linear difference is attained among the points evaluated. The
    /// node-only version of this test passed on a ladder codex measured at 0.323 m on a level
    /// declared at 5 cm; the difference is not a tolerance, it is a different quantity.
    ///
    /// A brute-force sub-cell sweep over one tile cross-checks that the overlay argument is not
    /// merely self-consistent — if the exact maximum were being missed, dense sampling would find a
    /// point above it.
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

    /// The overlay-maximum argument, CHECKED rather than asserted: a dense sub-cell sweep over the
    /// roughest tile must never find a point where the LOD surface departs from the canonical one by
    /// more than `worst_deviation_m` reported. Sampling cannot prove a maximum — but it can falsify
    /// one, and this is the test that would have caught the node-only version instantly.
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

    /// THE FINDING that shapes this module (memo FOLKLORE-RISK #11, resolved by measurement):
    /// **RTIN's error pyramid is a heuristic, not a bound.** An extraction at threshold τ can and
    /// does produce a mesh deviating by MORE than τ, because the pyramid measures each node against
    /// its own level's interpolation and those per-level errors compound down the hierarchy rather
    /// than dominating one another.
    ///
    /// This is why nothing in this module ever declares the extraction threshold. If the pyramid
    /// were ever tightened into a true bound the ratio below drops to 1.0 and this test still
    /// passes — it asserts the DIRECTION of the danger, not the number. The number is printed so
    /// the size of the gap on the shipped map is a recorded fact.
    #[test]
    fn the_error_pyramid_is_a_heuristic_and_the_declared_rung_is_the_measurement() {
        let grid = shipped_grid();
        let full = MESH_TILE_CELLS + 1;
        let hierarchy = rtin::Rtin::new(full);
        let mut worst_ratio = 0.0f32;
        let mut exceeded = 0usize;
        let mut extractions = 0usize;
        for range in mesh_tile_node_ranges(&grid) {
            let heights = patch(&grid, range);
            let errors = hierarchy.error_pyramid(&heights);
            for &threshold in &TERRAIN_LOD_LADDER[1..] {
                let tris = hierarchy.triangulate(&errors, threshold);
                let measured = worst_deviation_m(&heights, full, &tris);
                extractions += 1;
                if measured > threshold {
                    exceeded += 1;
                }
                worst_ratio = worst_ratio.max(measured / threshold);
            }
        }
        println!(
            "RTIN pyramid vs measurement: {exceeded}/{extractions} extractions exceed their own \
             threshold, worst ratio {worst_ratio:.3}×"
        );
        // The claim under test is that the builder does not TRUST the threshold. It holds whether
        // or not the pyramid happens to under-report on a given map, so the assertion is on the
        // builder's output, not on the ratio.
        let lod = build(&grid);
        for tile in &lod.tiles {
            for level in &tile.levels {
                assert!(
                    level.measured_dev_m <= TERRAIN_LOD_LADDER[level.rung],
                    "a declared rung must contain its own measurement"
                );
            }
        }
    }

    /// RTIN REMOVES vertices, it never moves one: every vertex of every level is an exact grid
    /// sample at its own world position — the pointwise form of ONE-SURFACE (codex finding 8).
    #[test]
    fn interior_vertices_are_exact_grid_samples() {
        let grid = shipped_grid();
        let lod = build(&grid);
        let step = crate::terrain_grid::WORLD_SIZE / (grid.size() - 1) as f32;
        let half = crate::terrain_grid::WORLD_HALF_EXTENT;
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

    /// NO SKIRTS, and no cracks either: every level of every tile carries the EXACT full-density
    /// border row on all four edges, so any level of one tile meets any level of its neighbour
    /// vertex-for-vertex. This is the whole crack strategy — if it holds, independent per-tile
    /// selection is safe; if it fails, tiles tear.
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

    /// The ranges a tile ships with must tile `[0, ∞)` exactly once: no distance at which the
    /// ground is drawn twice (z-fighting, doubled shadow cost) and none at which it vanishes.
    #[test]
    fn terrain_lod_ranges_are_complementary() {
        let lod = build(&shipped_grid());
        let view = TerrainLodView::default();
        for (t, tile) in lod.tiles.iter().enumerate() {
            let ranges = lod.ranges(tile, view);
            assert_eq!(ranges.len(), tile.levels.len());
            assert_eq!(
                ranges[0].start_margin.start, 0.0,
                "tile {t} must be drawn at the camera"
            );
            assert_eq!(
                ranges
                    .last()
                    .expect("a tile always has a level")
                    .end_margin
                    .end,
                f32::INFINITY,
                "tile {t} must never vanish"
            );
            for pair in ranges.windows(2) {
                assert_eq!(
                    pair[0].end_margin.end, pair[1].start_margin.start,
                    "tile {t} has a gap or an overlap in its range chain"
                );
                assert!(
                    pair[1].start_margin.start > pair[0].start_margin.start,
                    "tile {t} range chain is not strictly increasing"
                );
                assert!(pair[0].is_abrupt() && pair[1].is_abrupt());
            }
        }
    }

    /// COVERAGE survives a degenerate view profile. A headless or not-yet-sized window reports a
    /// zero pixel height, which collapses every switch distance onto the bounding radius: the
    /// middle levels then own EMPTY intervals. That is fine and must stay fine — every distance is
    /// still covered by exactly one level, so the ground never disappears and is never drawn
    /// twice. Asserted by evaluating the chain, not by trusting it to be strictly increasing.
    #[test]
    fn the_range_chain_covers_every_distance_even_at_a_degenerate_view() {
        let lod = build(&shipped_grid());
        for view in [
            TerrainLodView {
                fov_y_rad: std::f32::consts::FRAC_PI_4,
                height_px: 0.0,
                budget_px: TERRAIN_LOD_BUDGET_PX,
            },
            TerrainLodView::default(),
        ] {
            for tile in &lod.tiles {
                let ranges = lod.ranges(tile, view);
                for distance in [0.0, 1.0, 93.0, 94.2, 200.0, 999.0, 5_000.0, 100_000.0] {
                    let visible = ranges
                        .iter()
                        .filter(|range| range.is_visible_at_all(distance))
                        .count();
                    assert_eq!(
                        visible, 1,
                        "{visible} levels visible at {distance} m (view {view:?})"
                    );
                }
            }
        }
    }

    /// The DERIVATION pin: every wired threshold is re-computed here from the constants alone —
    /// ladder rung, pixel budget, view profile, bounding radius — and must equal what the ladder
    /// hands the ECS. A literal metre count creeping into the wiring fails here, and so does a
    /// changed budget that someone forgot to propagate.
    #[test]
    fn wired_thresholds_are_the_derivation() {
        let lod = build(&shipped_grid());
        let view = TerrainLodView {
            fov_y_rad: crate::camera::GUNNER_FOV_FALLBACK,
            height_px: 2160.0,
            budget_px: TERRAIN_LOD_BUDGET_PX,
        };
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
                    dev * view.height_px / (view.fov_y_rad * view.budget_px) + radius
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

    /// The CENSUS, reported (not gated): what the ladder actually costs on the shipped map — per
    /// rung, how many tiles keep it, how many triangles that is, and the worst measured deviation
    /// among them. Run with `--nocapture` to read it. The startup log prints the same line, so
    /// this is where the number is checked against a change without launching the game.
    #[test]
    fn terrain_lod_census_is_reported() {
        let grid = shipped_grid();
        let started = std::time::Instant::now();
        let lod = build(&grid);
        let wall = started.elapsed();
        let view = TerrainLodView {
            fov_y_rad: crate::camera::GUNNER_FOV_FALLBACK,
            height_px: 2160.0,
            budget_px: TERRAIN_LOD_BUDGET_PX,
        };
        let mut census = vec![(0usize, 0usize, 0.0f32); TERRAIN_LOD_LADDER.len()];
        for tile in &lod.tiles {
            for level in &tile.levels {
                let (tiles, tris, worst) = &mut census[level.rung];
                *tiles += 1;
                *tris += level.triangles;
                *worst = worst.max(level.measured_dev_m);
            }
        }
        println!(
            "terrain LOD census: {} tiles, radius {:.2} m, generated in {} ms",
            lod.tiles.len(),
            lod.bounding_radius_m,
            wall.as_millis(),
        );
        for (rung, (tiles, tris, worst)) in census.iter().enumerate() {
            if *tiles == 0 {
                continue;
            }
            println!(
                "  rung δ ≤ {:.2} m: {tiles} tiles, {tris} tris, worst measured δ {worst:.4} m, \
                 switch at {:.0} m",
                TERRAIN_LOD_LADDER[rung],
                view.switch_distance_m(rung, lod.bounding_radius_m),
            );
        }
        let entities: usize = lod.tiles.iter().map(|tile| tile.levels.len()).sum();
        let resident: usize = census.iter().map(|(_, tris, _)| tris).sum();
        println!("  {entities} entities, {resident} triangles resident across the whole pyramid");
        assert!(lod.tiles.iter().all(|tile| !tile.levels.is_empty()));
    }

    /// A two-level ladder with no geometry — enough to exercise selection and the adaptive layer
    /// without decoding a heightmap.
    fn synthetic_ladder() -> TerrainLod {
        let level = |rung: usize| TerrainLodLevel {
            rung,
            extracted_at_m: TERRAIN_LOD_LADDER[rung],
            measured_dev_m: TERRAIN_LOD_LADDER[rung],
            triangles: 0,
            mesh: Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            ),
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

    /// THE ADAPTIVE LAYER: switching to the gunner optic must pull every terrain switch distance
    /// OUT (a narrow field magnifies the same deviation, so the coarse level is only legal much
    /// further away), and returning to the commander view must pull them back in.
    ///
    /// This is the whole reason terrain gets the adaptive layer on day one and the shoe does not:
    /// a few hundred entities and a human-rate trigger. If it silently stopped firing, the ladder
    /// would be selected for whatever view happened to be live at startup — which is why this
    /// asserts the DIRECTION of the rewrite and not merely that something changed.
    #[test]
    fn the_adaptive_layer_rewrites_thresholds_when_the_view_changes() {
        let mut app = App::new();
        app.add_plugins(plugin);
        app.insert_resource(TerrainLodLadder(synthetic_ladder()));
        app.insert_resource(TerrainLodView {
            fov_y_rad: std::f32::consts::FRAC_PI_4,
            height_px: 1440.0,
            budget_px: TERRAIN_LOD_BUDGET_PX,
        });
        let world = app.world_mut();
        let window = world.spawn(Window::default()).id();
        let camera = world
            .spawn((
                Camera3d::default(),
                Projection::Perspective(PerspectiveProjection {
                    fov: std::f32::consts::FRAC_PI_4,
                    ..default()
                }),
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

        // A zero pixel height is an ABSENT viewport (a window bevy has not sized yet), not a
        // one-pixel one. Read literally it would collapse every threshold onto the bounding radius
        // and put the COARSEST level 94 m from the camera on the first frames a player sees.
        assert_eq!(
            TerrainLodView::new(0.5, 0.0).height_px,
            TerrainLodView::default().height_px,
            "an unsized window must not be read as a zero-pixel viewport"
        );

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

        // Into the optic: the same deviations now cost 6.5× more distance to hide.
        {
            let mut lens = app
                .world_mut()
                .get_mut::<Projection>(camera)
                .expect("the test camera");
            let Projection::Perspective(projection) = lens.as_mut() else {
                panic!("the test camera must carry a perspective projection");
            };
            projection.fov = crate::camera::GUNNER_FOV_FALLBACK;
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

    /// The STARTUP COST, measured: generation plus the mikktspace tangent basis every level needs —
    /// the two terms that land on the loading screen and nowhere else. Ignored by default because
    /// mikktspace over the whole pyramid is the heaviest thing in this module; run it
    /// (`cargo test --lib startup_cost -- --ignored --nocapture`) whenever the ladder, the tiling,
    /// or `MESH_TILE_CELLS` changes.
    ///
    /// It also serves as the tangent-basis certification for every level (ADR-0011): the pass
    /// panics on any level mikktspace cannot handle, and it is the SAME pass startup runs.
    #[test]
    #[ignore = "measurement: mikktspace over the whole pyramid is the module's heaviest step"]
    fn terrain_lod_startup_cost_is_measured() {
        let grid = shipped_grid();
        let mut lod = build(&grid);
        let triangles: usize = lod
            .tiles
            .iter()
            .flat_map(|tile| tile.levels.iter().map(|level| level.triangles))
            .sum();
        let started = std::time::Instant::now();
        generate_tangents(&mut lod);
        println!(
            "terrain LOD startup: generation {} ms + tangents {} ms over {triangles} triangles \
             on {} cores",
            lod.build.as_millis(),
            started.elapsed().as_millis(),
            std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
        );
        for tile in &lod.tiles {
            for level in &tile.levels {
                assert!(
                    level.mesh.attribute(Mesh::ATTRIBUTE_TANGENT).is_some(),
                    "every level must leave a tangent attribute"
                );
            }
        }
    }

    /// Small-angle conservatism, pinned with the numbers the doc comment claims: the wired form
    /// never switches to a coarser level SOONER than the exact projection would allow.
    #[test]
    fn the_small_angle_form_is_conservative_in_both_views() {
        for (fov, claim) in [(0.12_f32, 0.002_f32), (std::f32::consts::FRAC_PI_4, 0.06)] {
            let ours = sub_pixel_distance_m(0.05, 2160.0, fov, 1.0);
            let exact = 0.05 * 2160.0 / (2.0 * (fov / 2.0).tan());
            assert!(ours >= exact, "small-angle must not switch early at {fov}");
            assert!(
                (ours - exact) / exact <= claim,
                "small-angle overshoot at fov {fov} is {}, claimed ≤ {claim}",
                (ours - exact) / exact
            );
        }
    }
}

/// The TACTICAL harness (brief §6b, codex finding 1): what a coarser level does to a SIGHTLINE, as
/// opposed to what it does to a silhouette.
///
/// The whole positional ladder is built on projected vertical error, and codex's first finding is
/// that projected error cannot see motion ALONG a pixel ray. Near tangency the ray-surface
/// intersection satisfies `δt ≈ δh / (d_y − ∇h·d_xz)`, whose denominator goes to zero exactly where
/// tanks look at each other: a deviation that is invisible on screen can move a first-hit hundreds
/// of metres and decide whether a hull is behind a crest or on top of it.
///
/// So this module MEASURES two things the positional gate cannot:
/// * **first-hit distance error** — exact surface vs each level, over the tank/optic camera-height
///   envelope at grazing pitches;
/// * **crest-occlusion flips** — observer-to-target sightlines at hull height, counting the cases
///   where the two surfaces DISAGREE about whether the target is visible, split by direction
///   (a level that REVEALS a hull the exact ground hides is the dangerous one).
///
/// **PROVISIONAL — measurement now, gate later.** The budgets for these numbers are the one
/// genuinely new dial terrain LOD introduces and they are Yan's to set (the brief's proposal is
/// ≤ 5 m first-hit error at any legal engagement sightline and zero hull-revealing flips). Until
/// then `tactical_ray_harness_reports` runs, prints, and asserts only that it exercised the
/// surfaces — turning it into a gate is a one-line change once the numbers are ratified.
#[cfg(test)]
mod tactical {
    use super::*;
    use crate::terrain_grid::{WORLD_HALF_EXTENT, WORLD_SIZE, tests::shipped_grid};

    /// Marching step along a ray, metres. BOTH surfaces are sampled by the SAME marcher at this
    /// step, so the reported error is a difference between two SURFACES and the marching
    /// resolution cancels — it is not an exact caster compared against an approximate one. The
    /// residual is the risk of stepping over a crest thinner than a quarter-cell, which is below
    /// the grid's own 0.977 m resolution and therefore below what the surfaces can express.
    const MARCH_STEP_M: f32 = 0.25;

    /// The whole map's drawn surface at one rung: every tile at the coarsest level it keeps whose
    /// declared rung is within the given one — i.e. the ground a player actually sees once that
    /// rung's switch distance is passed.
    struct LodSurface {
        /// World-space triangles, read back from the levels' own meshes (the geometry that ships,
        /// not a re-derivation of it).
        tris: Vec<[Vec3; 3]>,
        /// The at-most-two triangles covering each unit grid cell, `u32::MAX` for empty. A cell
        /// centre can lie exactly ON a leaf triangle's hypotenuse, which is why there are two.
        cells: Vec<[u32; 2]>,
        cells_per_side: usize,
        step: f32,
    }

    impl LodSurface {
        fn new(grid: &HeightGrid, lod: &TerrainLod, rung: usize) -> Self {
            let cells_per_side = grid.size() as usize - 1;
            let step = WORLD_SIZE / cells_per_side as f32;
            let mut tris: Vec<[Vec3; 3]> = Vec::new();
            let mut cells = vec![[u32::MAX; 2]; cells_per_side * cells_per_side];
            for tile in &lod.tiles {
                let Some(level) = tile.levels.iter().rfind(|level| level.rung <= rung) else {
                    continue;
                };
                let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
                    level.mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                else {
                    panic!("a level must carry f32x3 positions");
                };
                let Some(Indices::U32(indices)) = level.mesh.indices() else {
                    panic!("a level must carry u32 indices");
                };
                let node = |p: &[f32; 3]| {
                    (
                        ((p[0] + WORLD_HALF_EXTENT) / step).round() as i64,
                        ((p[2] + WORLD_HALF_EXTENT) / step).round() as i64,
                    )
                };
                for tri in indices.chunks_exact(3) {
                    let p = [
                        positions[tri[0] as usize],
                        positions[tri[1] as usize],
                        positions[tri[2] as usize],
                    ];
                    let id = tris.len() as u32;
                    tris.push([
                        Vec3::from_array(p[0]),
                        Vec3::from_array(p[1]),
                        Vec3::from_array(p[2]),
                    ]);
                    // Which unit cells does this triangle cover? Work in DOUBLED node
                    // coordinates so a cell centre is the integer point (2i+1, 2j+1) and every
                    // inside test is exact integer arithmetic.
                    let (a, b, c) = (node(&p[0]), node(&p[1]), node(&p[2]));
                    let (ax, ay) = (a.0 * 2, a.1 * 2);
                    let (bx, by) = (b.0 * 2, b.1 * 2);
                    let (cx, cy) = (c.0 * 2, c.1 * 2);
                    let area2 = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
                    if area2 == 0 {
                        continue;
                    }
                    let lo_i = a.0.min(b.0).min(c.0).max(0);
                    let hi_i = a.0.max(b.0).max(c.0).min(cells_per_side as i64);
                    let lo_j = a.1.min(b.1).min(c.1).max(0);
                    let hi_j = a.1.max(b.1).max(c.1).min(cells_per_side as i64);
                    for j in lo_j..hi_j {
                        for i in lo_i..hi_i {
                            let (x, y) = (i * 2 + 1, j * 2 + 1);
                            let ea = (cx - bx) * (y - by) - (cy - by) * (x - bx);
                            let eb = (ax - cx) * (y - cy) - (ay - cy) * (x - cx);
                            let ec = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
                            let inside = if area2 > 0 {
                                ea >= 0 && eb >= 0 && ec >= 0
                            } else {
                                ea <= 0 && eb <= 0 && ec <= 0
                            };
                            if !inside {
                                continue;
                            }
                            let slot = &mut cells[(j as usize) * cells_per_side + i as usize];
                            if slot[0] == u32::MAX {
                                slot[0] = id;
                            } else if slot[1] == u32::MAX {
                                slot[1] = id;
                            }
                        }
                    }
                }
            }
            Self {
                tris,
                cells,
                cells_per_side,
                step,
            }
        }

        /// The DRAWN surface height at world `(x, z)`, or `None` off the map.
        fn height_at(&self, x: f32, z: f32) -> Option<f32> {
            if x.abs() > WORLD_HALF_EXTENT || z.abs() > WORLD_HALF_EXTENT {
                return None;
            }
            let n = self.cells_per_side;
            let i = (((x + WORLD_HALF_EXTENT) / self.step) as usize).min(n - 1);
            let j = (((z + WORLD_HALF_EXTENT) / self.step) as usize).min(n - 1);
            let mut fallback = None;
            for &id in &self.cells[j * n + i] {
                if id == u32::MAX {
                    continue;
                }
                let [a, b, c] = self.tris[id as usize];
                let area = (b.x - a.x) * (c.z - a.z) - (b.z - a.z) * (c.x - a.x);
                if area == 0.0 {
                    continue;
                }
                let inv = 1.0 / area;
                let wa = ((c.x - b.x) * (z - b.z) - (c.z - b.z) * (x - b.x)) * inv;
                let wb = ((a.x - c.x) * (z - c.z) - (a.z - c.z) * (x - c.x)) * inv;
                let wc = ((b.x - a.x) * (z - a.z) - (b.z - a.z) * (x - a.x)) * inv;
                let height = wa * a.y + wb * b.y + wc * c.y;
                fallback = Some(height);
                const EPS: f32 = -1.0e-4;
                if wa >= EPS && wb >= EPS && wc >= EPS {
                    return Some(height);
                }
            }
            fallback
        }
    }

    /// First hit of a ray against a height function, marched. `t` is in metres along a unit `dir`.
    fn march(
        height: &impl Fn(f32, f32) -> Option<f32>,
        origin: Vec3,
        dir: Vec3,
        t_max: f32,
    ) -> Option<f32> {
        let mut previous: Option<(f32, f32)> = None; // (t, ray_y − surface)
        let mut t = 0.0f32;
        while t <= t_max {
            let p = origin + dir * t;
            if let Some(h) = height(p.x, p.z) {
                let gap = p.y - h;
                if gap <= 0.0 {
                    // Linear crossing between the last clear sample and this one.
                    return Some(match previous {
                        Some((t0, g0)) if g0 > 0.0 => t0 + (t - t0) * g0 / (g0 - gap),
                        _ => t,
                    });
                }
                previous = Some((t, gap));
            } else {
                previous = None;
            }
            t += MARCH_STEP_M;
        }
        None
    }

    /// A deterministic LCG stream (no platform RNG), the same generator shape `terrain_grid`'s
    /// seeded pins use.
    fn lcg(seed: u64) -> impl FnMut() -> f32 {
        let mut state = seed;
        move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as u32) as f32 / u32::MAX as f32
        }
    }

    /// Median / p95 / max of a sample set (sorted in place).
    fn quantiles(samples: &mut [f32]) -> (f32, f32, f32) {
        if samples.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        samples.sort_by(f32::total_cmp);
        let at = |q: f32| samples[((samples.len() - 1) as f32 * q).round() as usize];
        (at(0.5), at(0.95), at(1.0))
    }

    /// PROVISIONAL measurement, not a gate. Prints, per rung:
    /// * grazing-ray first-hit distance error (median / p95 / max) and hit-disagreement counts;
    /// * crest-occlusion flips on hull-height sightlines, split into REVEALS (the LOD ground shows
    ///   a hull the exact ground hides — the tactically dangerous direction) and HIDES.
    ///
    /// Run it with `cargo test --lib tactical -- --nocapture`.
    #[test]
    fn tactical_ray_harness_reports() {
        let grid = shipped_grid();
        let lod = build(&grid);
        let exact = |x: f32, z: f32| grid.contains_xz(x, z).then(|| grid.height_at(x, z));
        // The tank/optic camera-height envelope: hull roof, commander's head, and the tallest
        // legal optic ride — the band every engagement sightline is drawn from.
        const EYE_HEIGHTS_M: [f32; 3] = [1.5, 2.2, 3.0];
        const RAYS: usize = 384;
        const SIGHTLINES: usize = 768;
        let view_optic = TerrainLodView {
            fov_y_rad: crate::camera::GUNNER_FOV_FALLBACK,
            height_px: 2160.0,
            budget_px: TERRAIN_LOD_BUDGET_PX,
        };
        let view_cmdr = TerrainLodView {
            fov_y_rad: std::f32::consts::FRAC_PI_4,
            ..view_optic
        };
        println!(
            "TACTICAL HARNESS (PROVISIONAL — budgets await ratification). Map {WORLD_SIZE} m, \
             marched at {MARCH_STEP_M} m, {RAYS} grazing rays × {} eye heights, {SIGHTLINES} \
             hull-height sightlines.",
            EYE_HEIGHTS_M.len()
        );
        let mut exercised = 0usize;
        for (rung, &declared) in TERRAIN_LOD_LADDER.iter().enumerate().skip(1) {
            let surface = LodSurface::new(&grid, &lod, rung);
            let drawn = |x: f32, z: f32| surface.height_at(x, z);

            // ---- grazing first-hit sweep -------------------------------------------------
            let mut next = lcg(0x9E37_79B9_7F4A_7C15 ^ rung as u64);
            let mut errors: Vec<f32> = Vec::new();
            let (mut both, mut only_exact, mut only_lod) = (0usize, 0usize, 0usize);
            let mut worst_at = 0.0f32;
            for _ in 0..RAYS {
                let x = (next() * 2.0 - 1.0) * WORLD_HALF_EXTENT * 0.95;
                let z = (next() * 2.0 - 1.0) * WORLD_HALF_EXTENT * 0.95;
                let azimuth = next() * std::f32::consts::TAU;
                // Grazing band: 0.1° to 4° below horizontal, where the first-hit denominator is
                // small and a centimetre of height is worth tens of metres of range.
                let pitch = 0.0017 + next() * 0.0681;
                for eye in EYE_HEIGHTS_M {
                    let origin = Vec3::new(x, grid.height_at(x, z) + eye, z);
                    let dir = Vec3::new(
                        pitch.cos() * azimuth.cos(),
                        -pitch.sin(),
                        pitch.cos() * azimuth.sin(),
                    )
                    .normalize();
                    let a = march(&exact, origin, dir, 1500.0);
                    let b = march(&drawn, origin, dir, 1500.0);
                    match (a, b) {
                        (Some(a), Some(b)) => {
                            both += 1;
                            errors.push((a - b).abs());
                            if (a - b).abs() > worst_at {
                                worst_at = (a - b).abs();
                            }
                        }
                        (Some(_), None) => only_exact += 1,
                        (None, Some(_)) => only_lod += 1,
                        (None, None) => {}
                    }
                }
            }
            exercised += both;
            let (median, p95, max) = quantiles(&mut errors);

            // ---- crest occlusion ----------------------------------------------------------
            let mut next = lcg(0xB5AD_4ECE_DA1C_E2A9 ^ rung as u64);
            const HULL_M: f32 = 1.8;
            let (mut reveals, mut hides, mut tested) = (0usize, 0usize, 0usize);
            for _ in 0..SIGHTLINES {
                let x = (next() * 2.0 - 1.0) * WORLD_HALF_EXTENT * 0.9;
                let z = (next() * 2.0 - 1.0) * WORLD_HALF_EXTENT * 0.9;
                let azimuth = next() * std::f32::consts::TAU;
                let range = 200.0 + next() * 800.0;
                let (tx, tz) = (x + range * azimuth.cos(), z + range * azimuth.sin());
                if !grid.contains_xz(tx, tz) {
                    continue;
                }
                let from = Vec3::new(x, grid.height_at(x, z) + 2.2, z);
                let to = Vec3::new(tx, grid.height_at(tx, tz) + HULL_M, tz);
                let span = to - from;
                let distance = span.length();
                let dir = span / distance;
                // Blocked = the surface interrupts the segment before the target.
                let blocked = |height: &dyn Fn(f32, f32) -> Option<f32>| {
                    march(&|x, z| height(x, z), from, dir, distance - 0.5).is_some()
                };
                tested += 1;
                match (blocked(&exact), blocked(&drawn)) {
                    (true, false) => reveals += 1,
                    (false, true) => hides += 1,
                    _ => {}
                }
            }

            println!(
                "  rung δ ≤ {declared:.2} m (switch: optic {optic:.0} m, commander {cmdr:.0} m)\n\
                 \x20   first-hit error over {both} grazing rays: median {median:.2} m, \
                 p95 {p95:.2} m, max {max:.2} m; hit only-exact {only_exact}, only-LOD {only_lod}\n\
                 \x20   crest occlusion over {tested} sightlines: {reveals} REVEAL a hull the \
                 exact ground hides, {hides} hide one it shows",
                optic = view_optic.switch_distance_m(rung, lod.bounding_radius_m),
                cmdr = view_cmdr.switch_distance_m(rung, lod.bounding_radius_m),
            );
        }
        assert!(
            exercised > RAYS,
            "the harness must actually intersect the surfaces ({exercised} paired hits)"
        );
    }
}
