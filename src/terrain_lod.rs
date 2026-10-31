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
//!    about vertices and NOT about the surface**, and the difference is the whole reason for the two
//!    mechanisms below: RTIN also changes cell CONNECTIVITY, splitting cells along the main diagonal
//!    where the canonical surface uses the anti-diagonal, and two tessellations can agree at every
//!    grid node while being different ground between them. So
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
//! # THE DOCTRINE, AND WHAT IT DELIBERATELY DOES NOT COVER
//!
//! **There is exactly ONE hard limit in the LOD doctrine, terrain and models alike: a level's
//! worst-case surface deviation must project under the pixel budget through the LIVE view.** That
//! is the whole gate. It is enforced here by `every_lod_level_stays_within_its_declared_deviation`
//! (the exact continuous maximum, not a sample) and by the switch distance every rung is wired at.
//!
//! Three channels are KNOWN to escape that bound. They are measured, recorded here, and
//! deliberately NOT handled — no special-case gate, no second threshold, no held-back rung. The
//! simplicity is the point: one bound that every asset class shares is worth more than a set of
//! per-channel rules nobody can hold in their head. All figures MEASURED on the shipped map,
//! 2026-08-02.
//!
//! 1. **Shadows.** While the terrain's sun shadow is the baked field (`terrain_shadow`), no level
//!    casts: the field reads the canonical grid. A level still RECEIVES at its own height, so an
//!    edge on it moves by its deviation times `cot 17°`, the same factor as below. What follows is
//!    the raster arm (`SPIKE_ABLATE=terrain-field`). Terrain casts under the shipped 17° sun, and
//!    the vendored `check_dir_light_mesh_visibility` consults `VisibilityRange` while collecting
//!    directional shadow casters, so a switched level casts the switched shadow — the shadow pass
//!    selects the same LOD the main camera did. A vertical occluder change `δh` moves its shadow
//!    edge horizontally by `δh·cot 17° ≈ 3.27 δh` on a flat receiver, and further where the
//!    ground runs near-parallel to the light — so the positional rung underprices it by 3.27×.
//!    DERIVED from the worst kept level (0.4946 m at the 50 cm rung) the edge can move ~1.6 m in
//!    world space. On screen it is much smaller, because a horizontal displacement seen from tank
//!    height is heavily foreshortened and because a level is only shown at or beyond the distance
//!    where its own deviation is sub-pixel: the worst edge shift reads as ~1–2 px on this map at
//!    `ShadowDistance` m1000, which is the setting at which the cascades reach the ranges where
//!    coarse levels are selected at all. Below m1000 there is nothing to see.
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
//! Looking at any of the three by eye: the rung cycler ([`TerrainLodClamp`]) pins the whole map
//! to one rung, which is the only way to judge a level that would otherwise only ever appear a
//! kilometre away. For the shadow channel, set `ShadowDistance` to m1000 first.
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

/// The DECLARED deviation ladder, metres — both the extraction schedule and the rung vocabulary a
/// measured level is quantized up into. A level carrying rung `r` lies within
/// `TERRAIN_LOD_LADDER[r]` of the canonical surface EVERYWHERE, not merely at grid nodes.
///
/// Rung 0 is the exact surface (every grid sample kept) and is generated by
/// [`crate::terrain_grid::terrain_mesh_tiles`], the same full-density path the collider-matched
/// ground has always used — the close-up picture is that surface and nothing else.
///
/// **The DELIVERED set on the shipped map is {0, 0.05, 0.10, 0.25, 0.50} m** (ratified). The 2 cm
/// entry survives as an extraction threshold only: it produces a finer triangulation on some tiles,
/// but no tile's measured deviation quantizes down to it, so no level ever carries that rung. Drop
/// it and those tiles lose their 5 cm level; keep it and it costs one extraction pass. The rungs are
/// a shared error VOCABULARY, not a density schedule — a tile carries only what produces a distinct
/// mesh on it.
pub(crate) const TERRAIN_LOD_LADDER: [f32; 6] = [0.0, 0.02, 0.05, 0.10, 0.25, 0.50];

/// Screen-space error budget, PIXELS: the on-screen size a level's deviation is allowed to project
/// to before the next-finer level must take over.
///
/// THE SEAM: when `settings` grows a player-facing LOD-quality row this becomes its value. Until
/// then it is one constant with one home, and the whole selection chain derives from it — the
/// derivation test re-computes every wired threshold from this number and fails if a literal ever
/// creeps into the wiring.
pub(crate) const TERRAIN_LOD_BUDGET_PX: f32 = 1.0;

/// The view this ladder selects through: the shared live view (`crate::view`) spent at
/// [`TERRAIN_LOD_BUDGET_PX`].
///
/// The FACTS have one home and one writer for the whole tree; the BUDGET is this ladder's own
/// tuning knob. Composed rather than stored, so there is nothing here that can drift from what the
/// tank's chains are being selected against.
pub(crate) fn terrain_view(facts: ViewFacts) -> ViewProfile {
    ViewProfile::of(facts, TERRAIN_LOD_BUDGET_PX)
}

/// The distance (metres, camera → tile CENTRE) at or beyond which rung `rung` is legal for a tile
/// whose bounding radius is `radius_m`.
///
/// Two terms, and both are load-bearing:
/// * the exact projection ([`ViewFacts::sub_pixel_distance_m`]) — when the rung's declared
///   deviation stops being worth a pixel;
/// * `+ radius_m` — because `use_aabb` measures to the tile's CENTRE (verified in the vendored
///   source, see the module doc) while the deviation lives on the tile's nearest SURFACE, which can
///   be a full bounding radius closer to the camera. One shared radius (the largest over all tiles)
///   rather than a per-tile float, so the range table stays small.
///
/// Rung 0 is the exact surface: it starts at the camera, always.
///
/// A FREE FUNCTION, not a method: the ladder holds the radius and is borrowed mutably while its
/// levels are spawned, and the profile is a value composed on the spot.
pub(crate) fn rung_switch_distance_m(view: ViewProfile, rung: usize, radius_m: f32) -> f32 {
    if rung == 0 {
        return 0.0;
    }
    view.switch_distance_m(TERRAIN_LOD_LADDER[rung], radius_m)
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
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, TERRAIN_MESH_USAGE);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// The EXACT continuous worst deviation between a triangulation and THE canonical surface over the
/// tile, metres — not a sampled statistic.
///
/// # Why NODE sampling is not enough, and why this is exact
///
/// Sampling grid nodes would be enough if the two triangulations differed only in which vertices
/// they keep. They do not: they also differ in cell CONNECTIVITY. The canonical surface splits every
/// cell along the ANTI-diagonal (`HeightGrid::height_at`, and parry's own triangulation with it),
/// while RTIN's alternating bintree gives roughly half its leaf cells the MAIN diagonal. Those are
/// two triangulations of the same four corner heights, agreeing at all four nodes and disagreeing by
/// half the cell's twist `|h₀₀ + h₁₁ − h₁₀ − h₀₁| / 2` at the centre — MEASURED at 0.323 m in one
/// shipped cell, twelve times what node sampling reports there.
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
        /// # Why this exists
        ///
        /// The canonical surface splits every cell along the anti-diagonal — `HeightGrid::height_at`
        /// evaluates it and parry's heightfield triangulates it, and the ONE-SURFACE invariant is
        /// that claim. RTIN's bintree does NOT: its leaf orientation is fixed by position parity,
        /// so roughly half of its leaf cells come out on the MAIN diagonal. The two agree at all
        /// four corners and disagree by half the cell's twist, `|h₀₀ + h₁₁ − h₁₀ − h₀₁| / 2`, at the
        /// centre — invisible to node sampling, MEASURED at 0.323 m in one shipped cell.
        ///
        /// Constraining the hierarchy itself to the anti-diagonal is not possible: the alternating
        /// bintree's leaf parity is structural, not a choice at extraction time. It does not need
        /// to be — a cell covered by exactly TWO leaf triangles can be re-split locally, because
        /// both splittings have the IDENTICAL four boundary edges and the identical vertex set. Only
        /// the interior diagonal changes, so nothing outside the cell can observe it: no crack, no
        /// T-junction, no change in triangle count.
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
///
/// `view` IS AN OPTION because the ladder is spawned at Startup, before anything has read the
/// declared camera — and there is no default view to seed it with (`crate::view`). NONE spawns
/// every tile at its FINEST level, owning every distance, with the coarser ones out of range: the
/// authored surface, over-detailed rather than under-detailed, until [`adapt_ranges`] rewrites
/// every threshold on the first frame that has facts. A seeded profile would be a guess at the
/// player's field and screen, and a guess is what this whole shape removes.
///
/// Every level carries `scope`: [`VisualScope::BAKED_CASTER`] when the terrain's sun shadow is the
/// baked field (`terrain_shadow`), so no level casts into the cascades, or
/// [`VisualScope::WORLD_SOLID`] when the cascades carry it.
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
    generate_tangents(&mut lod);
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
            // The mesh (already tangented above) is MOVED out of the ladder here — what stays
            // behind is the census, not the geometry, because the whole pyramid resident twice is
            // ~200 MB of nothing.
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

/// Which level the ladder is currently showing. [`TerrainLodClamp::Adaptive`] is the product
/// behaviour and the default; every other value pins the whole map to one rung so a human can look
/// at that rung directly.
///
/// The clamp exists because two of this ladder's properties are not testable from a terminal: what
/// a rung looks like in motion, and what its SHADOW does under the shipped low sun (the one gate
/// this module still owes — see the module doc). Both need eyes on a running frame, and switching
/// them in at their real distances puts them a kilometre away where nobody can judge them.
///
/// Only the rung cycler's key ([`CLAMP_CYCLE_KEY`]) moves this off `Adaptive`; until it is pressed
/// the resource is read once per profile change and never written.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum TerrainLodClamp {
    /// Live selection: every tile picks its level from the camera distance.
    #[default]
    Adaptive,
    /// Every tile pinned to one rung of [`TERRAIN_LOD_LADDER`], regardless of distance.
    Rung(usize),
}

impl TerrainLodClamp {
    /// The level index this clamp selects on `tile`, and whether that was a COARSER fallback.
    ///
    /// A tile only carries the rungs that produced a distinct mesh on it, so a forced rung is often
    /// one the tile does not keep. The fallback is the nearest KEPT rung that is COARSER, never
    /// finer: a coarser level is what this tile shows a little further out anyway, so the clamped
    /// picture stays a picture the ladder can actually produce. If the tile keeps nothing that
    /// coarse, its coarsest level is the closest thing to the request.
    fn level_on(self, tile: &TerrainLodTile) -> Option<(usize, bool)> {
        let Self::Rung(rung) = self else {
            return None;
        };
        let exact = tile.levels.iter().position(|level| level.rung == rung);
        Some(match exact {
            Some(level) => (level, false),
            None => (
                tile.levels
                    .iter()
                    .position(|level| level.rung > rung)
                    .unwrap_or(tile.levels.len() - 1),
                true,
            ),
        })
    }
}

/// A range that no distance satisfies: `is_visible_at_all` needs `d >= start && d < end`, which
/// `∞ ..∞` can never give. How the clamp hides the levels it is not showing, without touching the
/// ladder's own thresholds — one extra value in bevy's permanent range table, on dev builds only.
fn never_visible() -> VisibilityRange {
    VisibilityRange {
        start_margin: f32::INFINITY..f32::INFINITY,
        end_margin: f32::INFINITY..f32::INFINITY,
        use_aabb: true,
    }
}

/// THE ONE WRITER of terrain visibility ranges. Rewrites every threshold when the shared view moves
/// — fov (the optic toggle, a magnification step), rendered height (window resize, render scale) —
/// or when the dev clamp moves.
///
/// NOT A READER OF THE VIEW. `view::track_view_facts` owns that, dead band included; this system
/// consumes [`ViewFacts`] and pairs it with this ladder's own budget. Affordable BECAUSE terrain is
/// a few hundred entities and both triggers are human-rate. The thresholds stay octave-shared: this
/// writes at most one distinct value per (rung, profile), never a per-tile float, so the permanent
/// range table grows by a handful of slots per profile the player actually visits.
///
/// The clamp rides THIS path rather than a parallel one on purpose. A second writer would race this
/// one for the same components, and whichever ran last would win a frame at a time — the clamp would
/// appear to work and then flicker back the moment the player toggled the optic.
fn adapt_ranges(
    mut commands: Commands,
    ladder: Option<Res<TerrainLodLadder>>,
    facts: Res<ViewFacts>,
    clamp: Res<TerrainLodClamp>,
    // The applied PROFILE, not the live one: a ladder spawned before the view is declared carries
    // every tile at its finest level, so the first frame with facts must rewrite even when nothing
    // has moved since.
    mut applied: Local<Option<(ViewProfile, TerrainLodClamp)>>,
    entities: Query<(Entity, &TerrainLodEntity)>,
) {
    let Some(ladder) = ladder else {
        return;
    };
    let wanted = terrain_view(*facts);
    let clamp_moved = applied.is_none_or(|(_, applied)| applied != *clamp);
    let view_moved = applied.is_none_or(|(applied, _)| applied != wanted);
    if !view_moved && !clamp_moved {
        return;
    }
    *applied = Some((wanted, *clamp));
    let (mut pinned, mut fallback) = (0usize, 0usize);
    for (entity, marker) in &entities {
        let Some(tile) = ladder.0.tiles.get(marker.tile) else {
            continue;
        };
        let range = match clamp.level_on(tile) {
            Some((level, coarser)) => {
                if marker.level == 0 {
                    pinned += 1;
                    fallback += usize::from(coarser);
                }
                if marker.level == level {
                    VisibilityRange::abrupt(0.0, f32::INFINITY)
                } else {
                    never_visible()
                }
            }
            None => match ladder.0.ranges(tile, wanted).get(marker.level) {
                Some(range) => range.clone(),
                None => continue,
            },
        };
        commands.entity(entity).insert(range);
    }
    match *clamp {
        TerrainLodClamp::Adaptive if clamp_moved => info!(
            "terrain LOD clamp: ADAPTIVE — live thresholds restored (fov {fov:.3} rad × \
             {height:.0} px)",
            fov = wanted.facts.vfov_rad,
            height = wanted.facts.height_px,
        ),
        TerrainLodClamp::Adaptive => info!(
            "terrain LOD: view profile → fov {fov:.3} rad × {height:.0} px, thresholds rewritten",
            fov = wanted.facts.vfov_rad,
            height = wanted.facts.height_px,
        ),
        TerrainLodClamp::Rung(0) => {
            info!("terrain LOD clamp: EXACT ({pinned} tiles at full density)")
        }
        TerrainLodClamp::Rung(rung) => info!(
            "terrain LOD clamp: δ{declared:.2} ({at} tiles at their {declared:.2} level, \
             {fallback} coarser-fallback)",
            declared = TERRAIN_LOD_LADDER[rung],
            at = pinned - fallback,
        ),
    }
}

/// Mount the adaptive layer, and on dev builds the rung cycler. Generation itself is driven by
/// `world::spawn_environment`.
///
/// `world::plugin` mounts this module on the DEDICATED SERVER too, which has no window, no camera
/// and therefore no `view::plugin` — and no ladder either, since nothing there spawns terrain
/// meshes. The run condition states that: a composition with no live view has no selection to make.
pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TerrainLodClamp>().add_systems(
        Update,
        adapt_ranges
            .run_if(resource_exists::<ViewFacts>)
            // Same frame as the resize that caused it, not the next one.
            .after(crate::view::track_view_facts),
    );
    // The eyeball rig, in every build: until its key is pressed, `TerrainLodClamp` stays
    // `Adaptive` and the resource is inert.
    app.add_systems(Update, cycle_clamp.before(adapt_ranges));
}

/// The dev key that cycles the terrain rung clamp: ADAPTIVE → EXACT → each rung the map actually
/// keeps → back to ADAPTIVE.
///
/// `K` because it is free — `G` gizmos, `X` the x-ray, `F` camera detach, `M` spawn map, `T`
/// optic, `L` and the digits are taken (`debug`, `command`, `sight`, `net::spawn_map`).
const CLAMP_CYCLE_KEY: KeyCode = KeyCode::KeyK;

/// Step the clamp on each press of [`CLAMP_CYCLE_KEY`].
///
/// The cycle visits EXACT plus every rung at least one tile keeps, read off the built ladder rather
/// than off [`TERRAIN_LOD_LADDER`] — a rung no tile carries would be a press that changes nothing,
/// which is worse than no key at all when the whole point is to see the difference.
fn cycle_clamp(
    // OPTIONAL because `world::plugin` mounts this module on the dedicated server too, and a
    // headless composition has no `InputPlugin` and therefore no keyboard resource at all.
    keys: Option<Res<ButtonInput<KeyCode>>>,
    ladder: Option<Res<TerrainLodLadder>>,
    mut clamp: ResMut<TerrainLodClamp>,
    mut explained: Local<bool>,
) {
    let (Some(keys), Some(ladder)) = (keys, ladder) else {
        return;
    };
    if !keys.just_pressed(CLAMP_CYCLE_KEY) {
        return;
    }
    let mut rungs: Vec<usize> = ladder
        .0
        .tiles
        .iter()
        .flat_map(|tile| tile.levels.iter().map(|level| level.rung))
        .collect();
    rungs.sort_unstable();
    rungs.dedup();
    *clamp = match *clamp {
        TerrainLodClamp::Adaptive => rungs.first().map_or(TerrainLodClamp::Adaptive, |rung| {
            TerrainLodClamp::Rung(*rung)
        }),
        TerrainLodClamp::Rung(rung) => rungs
            .iter()
            .find(|kept| **kept > rung)
            .map_or(TerrainLodClamp::Adaptive, |rung| {
                TerrainLodClamp::Rung(*rung)
            }),
    };
    if !*explained && matches!(*clamp, TerrainLodClamp::Rung(_)) {
        *explained = true;
        info!(
            "terrain LOD clamp: every tile is pinned to the requested rung; a tile that does not \
             keep that rung shows its nearest COARSER kept level, which is what it would show a \
             little further out anyway. Press {CLAMP_CYCLE_KEY:?} to step, and again past the \
             coarsest rung to return to ADAPTIVE."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A MEASURED view — the only kind there is (`crate::view`: no default view). Every fixture
    /// below states real numbers, so the `expect` is the honest reading of the constructor.
    fn measured(vfov_rad: f32, height_px: f32) -> ViewFacts {
        ViewFacts::new(vfov_rad, height_px).expect("a measured view")
    }

    /// The reference view the shipped corpus was cut against: the gunner optic at 4K native.
    fn reference_facts() -> ViewFacts {
        measured(crate::camera::GUNNER_FOV_FALLBACK, 2160.0)
    }

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

    /// THE sanity gate, and it is a CONNECTIVITY claim rather than a count: the extraction at a
    /// negative threshold must reproduce not merely `2 · 128²` triangles per tile, but the IDENTICAL
    /// triangle set `terrain_mesh_tiles` builds — same triples, same winding.
    ///
    /// Counting alone cannot see the failure that matters: RTIN's alternating bintree emits the
    /// right NUMBER of leaf triangles while splitting roughly half its cells along the main diagonal
    /// instead of parry's anti-diagonal, which is a different SURFACE with identical corner heights.
    /// A set comparison against the canonical tessellation does see it, and it is what pins
    /// `canonicalize_cell_diagonals` to the ONE-SURFACE invariant.
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
        assert_eq!(total, 2 * 1024 * 1024, "2,097,152 triangles over the map");
    }

    /// THE POSITIONAL CERTIFICATION: every kept level, on every tile of the shipped map, must lie
    /// within its DECLARED rung of the CONTINUOUS canonical surface — not merely at grid nodes. The
    /// declared rung is what the switch distance is computed from, so a level that quietly exceeds
    /// it anywhere is a level shown too close.
    ///
    /// Certified against the OVERLAY maximum (see `worst_deviation_m`): grid nodes AND cell centres,
    /// which together contain every vertex of the common refinement of the two triangulations, so
    /// the maximum of their piecewise-linear difference is attained among the points evaluated. The
    /// node-only form is not a looser tolerance on the same quantity, it is a different quantity:
    /// it passes a level whose true deviation is 0.323 m while declaring 5 cm.
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
    ///
    /// Across the legal fov interval up to its open ends, and at a DEGENERATE profile: a headless
    /// or not-yet-sized window reports a zero pixel height, which collapses every switch distance
    /// onto the bounding radius so the middle levels own EMPTY intervals. Coverage must survive
    /// that, so the chain is checked by adjacency at every profile and is strictly increasing only
    /// where the profile has pixels.
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

    /// The DERIVATION pin: every wired threshold is re-computed here from the constants alone —
    /// ladder rung, pixel budget, view profile, bounding radius — and must equal what the ladder
    /// hands the ECS. A literal metre count creeping into the wiring fails here, and so does a
    /// changed budget that someone forgot to propagate.
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

    /// A two-level ladder with no geometry — enough to exercise selection and the adaptive layer
    /// without decoding a heightmap.
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
        // The SHARED view alongside the ladder: this module no longer reads a camera or a window,
        // so without `view::plugin` there is nothing for it to consume.
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
                    fov: std::f32::consts::FRAC_PI_4,
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

    /// `VisibilityRange` is not `Debug`, so comparisons quote its four margin bounds instead.
    fn bounds(range: &VisibilityRange) -> [f32; 4] {
        [
            range.start_margin.start,
            range.start_margin.end,
            range.end_margin.start,
            range.end_margin.end,
        ]
    }

    /// A three-level synthetic tile plus the ECS scaffolding the rewrite system needs.
    fn clamp_fixture() -> (App, Vec<Entity>) {
        let mut app = App::new();
        app.add_plugins((crate::view::plugin, plugin));
        app.insert_resource(TerrainLodLadder(synthetic_ladder()));
        let world = app.world_mut();
        world.spawn((Window::default(), bevy::window::PrimaryWindow));
        world.spawn((
            Camera3d::default(),
            Projection::Perspective(PerspectiveProjection {
                fov: std::f32::consts::FRAC_PI_4,
                ..default()
            }),
            crate::view::PlayerView,
        ));
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
        (app, entities)
    }

    /// The clamp is INERT until it is engaged: at its default, `Adaptive`, the ladder writes exactly
    /// the adaptive thresholds. Until the rung cycler's key is pressed, that is the whole of the
    /// clamp's effect.
    ///
    /// Engaged, the clamp pins every tile to ONE level: the requested rung shows over `[0, ∞)` and
    /// every other level of the tile is given a range no distance can satisfy. Returning to
    /// `Adaptive` restores the live thresholds — the clamp must not be a one-way door.
    #[test]
    fn the_rung_clamp_pins_one_level_and_releases_cleanly() {
        let (mut app, entities) = clamp_fixture();
        assert_eq!(
            *app.world().resource::<TerrainLodClamp>(),
            TerrainLodClamp::Adaptive,
            "the clamp must default to live selection"
        );
        let ladder = synthetic_ladder();
        let assert_adaptive = |app: &App, when: &str| {
            let view = terrain_view(*app.world().resource::<ViewFacts>());
            let expected = ladder.ranges(&ladder.tiles[0], view);
            for (level, &entity) in entities.iter().enumerate() {
                assert_eq!(
                    bounds(app.world().get::<VisibilityRange>(entity).expect("range")),
                    bounds(&expected[level]),
                    "level {level} must carry its adaptive threshold {when}"
                );
            }
        };
        app.update();
        assert_adaptive(&app, "before any clamp");
        // The synthetic tile keeps rungs 0, 2 and 4. Ask for 4 — its own level — then for 3, which
        // it does not keep and must answer with the nearest COARSER kept level, also 4.
        for (rung, wanted) in [(4usize, 2usize), (3, 2), (0, 0)] {
            *app.world_mut().resource_mut::<TerrainLodClamp>() = TerrainLodClamp::Rung(rung);
            app.update();
            for (level, &entity) in entities.iter().enumerate() {
                let range = app.world().get::<VisibilityRange>(entity).expect("range");
                if level == wanted {
                    assert_eq!(
                        bounds(range),
                        bounds(&VisibilityRange::abrupt(0.0, f32::INFINITY)),
                        "rung {rung} must show level {wanted} everywhere"
                    );
                } else {
                    assert!(
                        !range.is_visible_at_all(0.0)
                            && !range.is_visible_at_all(500.0)
                            && !range.is_visible_at_all(f32::MAX),
                        "rung {rung} must hide level {level} at every distance"
                    );
                }
            }
        }
        // And back: the live chain returns unchanged.
        *app.world_mut().resource_mut::<TerrainLodClamp>() = TerrainLodClamp::Adaptive;
        app.update();
        assert_adaptive(&app, "after the release");
    }

    /// The fallback rule, on the shipped ladder rather than a fixture: for every rung, every tile
    /// resolves to exactly one kept level, and a tile that does not keep the rung resolves to a
    /// COARSER one — never a finer one, which would show detail the clamp was asked to hide.
    #[test]
    fn the_clamp_falls_back_to_coarser_levels_only() {
        let lod = build(&shipped_grid());
        for rung in 0..TERRAIN_LOD_LADDER.len() {
            for (t, tile) in lod.tiles.iter().enumerate() {
                let (level, coarser) = TerrainLodClamp::Rung(rung)
                    .level_on(tile)
                    .expect("a pinned clamp always resolves");
                let kept = tile.levels[level].rung;
                assert_eq!(
                    coarser,
                    kept != rung,
                    "tile {t} rung {rung}: fallback flag disagrees with the level it chose"
                );
                if coarser {
                    assert!(
                        kept > rung || level == tile.levels.len() - 1,
                        "tile {t} rung {rung} fell back to the FINER level {kept}"
                    );
                }
            }
        }
        // Adaptive resolves to no level at all — that is what makes it the live path.
        assert!(TerrainLodClamp::Adaptive.level_on(&lod.tiles[0]).is_none());
    }

    /// Terrain meshes are RENDER-WORLD ONLY. The ground never changes after startup and nothing in
    /// the main world reads its vertices back, so the default flags' permanent CPU copy — DERIVED at
    /// ~100 MiB across this ladder — buys nothing. Pinned because the cost of losing it is invisible.
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
