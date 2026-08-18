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
//! 1. **Shadows.** Terrain casts under the shipped 17° sun, and the vendored
//!    `check_dir_light_mesh_visibility` consults `VisibilityRange` while collecting directional
//!    shadow casters, so a switched level casts the switched shadow — the shadow pass selects the
//!    same LOD the main camera did. A vertical occluder change `δh` moves its shadow edge
//!    horizontally by `δh·cot 17° ≈ 3.27 δh` on a flat receiver, and further where the ground runs
//!    near-parallel to the light — so the positional rung underprices it by 3.27×. DERIVED from the
//!    worst kept level (0.4946 m at the 50 cm rung) the edge can move ~1.6 m in world space. On
//!    screen it is much smaller, because a horizontal displacement seen from tank height is heavily
//!    foreshortened and because a level is only shown at or beyond the distance where its own
//!    deviation is sub-pixel: the worst edge shift reads as ~1–2 px on this map at `ShadowDistance`
//!    m1000, which is the setting at which the cascades reach the ranges where coarse levels are
//!    selected at all. Below m1000 there is nothing to see.
//!
//! 2. **Grazing-LOS first-hit slide.** Projected error cannot see motion ALONG a pixel ray. Near
//!    tangency the intersection satisfies `δt ≈ δh / (d_y − ∇h·d_xz)`, whose denominator goes to
//!    zero exactly where tanks look at each other. MEASURED worst first-hit displacement over 1146
//!    paired grazing hits: 0.000 m in the gunner optic (no coarse level is ever selected there on a
//!    1 km map), 0.507 m in the commander view at the shipped 1 px budget, and 166.098 m at an 8 px
//!    budget. Mostly DEPTH displacement — the hit slides toward or away from the eye along the line
//!    of sight, where it subtends almost no angle — which is why metres of it can be invisible.
//!    `tactical_ray_harness_reports` reprints all three every run.
//!
//! 3. **Crest occlusion.** MEASURED over 753 hull-height sightlines per profile: zero flips in
//!    either direction at 1 px, and one sightline where the coarse ground REVEALS a hull the exact
//!    ground hides at 8 px. A budget sweep at the commander field put the transition between 4 px
//!    (zero) and 5 px (one). Sub-pixel doctrine deliberately does not gate this — re-measure if it
//!    ever matters.
//!
//! The tactical harness (`mod tactical`) runs in the ordinary suite and prints all of the above
//! every time. It asserts only that the report is structurally sound — the harness ran, the caster
//! is pinned against an independent oracle, the coverage floor held. It asserts no tactical value.
//!
//! Looking at any of the three by eye: the `dev_tools` rung cycler ([`TerrainLodClamp`]) pins the
//! whole map to one rung, which is the only way to judge a level that would otherwise only ever
//! appear a kilometre away. For the shadow channel, set `ShadowDistance` to m1000 first.
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
pub(crate) fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: &Handle<crate::terrain_blend::TerrainMaterial>,
    grid: &HeightGrid,
    view: ViewProfile,
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
            .map(|level| rung_switch_distance_m(view, level.rung, radius))
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
            switch = rung_switch_distance_m(view, rung, radius),
        );
    }
    info!(
        "terrain LOD: {tiles} tiles → {entities} entities, generated in {gen} ms \
         (+{tan} ms tangents), radius {radius:.1} m, view fov {fov:.3} rad × {height:.0} px \
         @ {budget} px budget",
        tiles = lod.tiles.len(),
        gen = lod.build.as_millis(),
        tan = tangents.as_millis(),
        fov = view.facts.vfov_rad,
        height = view.facts.height_px,
        budget = view.budget_px,
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
/// Only `dev_tools` mounts anything that can move this off `Adaptive`, so on a shipped client it is
/// a resource that is read once per profile change and never written.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum TerrainLodClamp {
    /// Live selection: every tile picks its level from the camera distance.
    #[default]
    Adaptive,
    /// Every tile pinned to one rung of [`TERRAIN_LOD_LADDER`], regardless of distance.
    ///
    /// Unconstructible without `dev_tools`, and the dead-code warning that produces on a release
    /// build is the shipped-client guarantee stated by the compiler: nothing there can move the
    /// clamp off `Adaptive`.
    #[cfg_attr(
        not(feature = "dev_tools"),
        expect(
            dead_code,
            reason = "only the dev_tools rung cycler constructs this; see the variant doc"
        )
    )]
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
    // The applied PROFILE, not the live one: the ladder is spawned against a Startup seed (the
    // narrow-optic guess in `world`), so the first frame with real facts must rewrite even when
    // nothing has moved since.
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
    // The eyeball rig. `dev_tools` is a DEFAULT feature, so this ships to playtest builds and not
    // to a release client; without it nothing can move `TerrainLodClamp` off `Adaptive` and the
    // resource is inert.
    #[cfg(feature = "dev_tools")]
    app.add_systems(Update, cycle_clamp.before(adapt_ranges));
}

/// The dev key that cycles the terrain rung clamp: ADAPTIVE → EXACT → each rung the map actually
/// keeps → back to ADAPTIVE.
///
/// `K` because it is free — `G` gizmos, `X` x-ray, `F` camera detach, `M` spawn map, `T` optic, `L`
/// and the digits are taken (`debug`, `command`, `sight`, `net::spawn_map`).
#[cfg(feature = "dev_tools")]
const CLAMP_CYCLE_KEY: KeyCode = KeyCode::KeyK;

/// Step the clamp on each press of [`CLAMP_CYCLE_KEY`].
///
/// The cycle visits EXACT plus every rung at least one tile keeps, read off the built ladder rather
/// than off [`TERRAIN_LOD_LADDER`] — a rung no tile carries would be a press that changes nothing,
/// which is worse than no key at all when the whole point is to see the difference.
#[cfg(feature = "dev_tools")]
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
    /// `terrain_mesh_tiles` is expressed through helpers it shares with the ladder
    /// (`node_world_coord`, `surface_normal_at`, `mesh_tile_node_ranges`), so a change to any of them
    /// is a change to the ONE surface every consumer reads. Every vertex, normal, UV and index of
    /// every shipped tile is therefore recomputed here from the raw expressions, no helper called,
    /// plus a content hash over the whole set as the cheap tripwire for future diffs.
    #[test]
    fn level_zero_is_the_pre_lod_tiler_bit_for_bit() {
        use crate::terrain_grid::TEXTURE_TILE_M;
        let grid = shipped_grid();
        let n = grid.size() as usize;
        let cells = n - 1;
        let step = grid.world_size() / cells as f32;
        let half = grid.half_extent();
        let world_at = |k: usize| -half + k as f32 * step;
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
        // Re-pinned when the decode began honouring the map's declared row order
        // (`terrain_grid::RowOrder`): the same samples, reversed row-wise, are a different surface.
        assert_eq!(
            hash, 0x4063_c4c4_5e28_ed45,
            "the level-zero surface changed; the assertions above name which part"
        );
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

    /// **RTIN's error pyramid is a HEURISTIC, not a bound**, and that fact shapes this whole
    /// module. An extraction at threshold τ can and does produce a mesh deviating by MORE than τ,
    /// because the pyramid measures each node against its own level's interpolation and those
    /// per-level errors compound down the hierarchy rather than dominating one another.
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
    #[test]
    fn terrain_lod_ranges_are_complementary() {
        let lod = build(&shipped_grid());
        let view = terrain_view(ViewFacts::default());
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
            // Built FIELD-BY-FIELD rather than through `ViewFacts::new`, whose whole job is to
            // refuse this: the point is that the range chain survives a degenerate profile even if
            // one ever reaches it.
            ViewProfile {
                facts: ViewFacts {
                    vfov_rad: std::f32::consts::FRAC_PI_4,
                    height_px: 0.0,
                },
                budget_px: TERRAIN_LOD_BUDGET_PX,
            },
            terrain_view(ViewFacts::default()),
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
        let view = terrain_view(ViewFacts::new(crate::camera::GUNNER_FOV_FALLBACK, 2160.0));
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
        let view = terrain_view(ViewFacts::new(crate::camera::GUNNER_FOV_FALLBACK, 2160.0));
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
                rung_switch_distance_m(view, rung, lod.bounding_radius_m),
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
        world.spawn(Window::default());
        world.spawn((
            Camera3d::default(),
            Projection::Perspective(PerspectiveProjection {
                fov: std::f32::consts::FRAC_PI_4,
                ..default()
            }),
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

    /// The clamp is INERT until it is engaged: with the resource at its default the ladder writes
    /// exactly the adaptive thresholds it would write with no clamp in the code at all.
    ///
    /// This is also the shipped-client claim. `dev_tools` is the only thing that mounts a system
    /// able to move `TerrainLodClamp`, so on a release build the resource is permanently `Adaptive`
    /// and what this test asserts is the whole of the clamp's effect there: none.
    #[test]
    fn the_rung_clamp_is_inert_until_it_is_engaged() {
        let (mut app, entities) = clamp_fixture();
        assert_eq!(
            *app.world().resource::<TerrainLodClamp>(),
            TerrainLodClamp::Adaptive,
            "the clamp must default to live selection"
        );
        app.update();
        let ladder = synthetic_ladder();
        let view = terrain_view(*app.world().resource::<ViewFacts>());
        let expected = ladder.ranges(&ladder.tiles[0], view);
        for (level, &entity) in entities.iter().enumerate() {
            assert_eq!(
                bounds(app.world().get::<VisibilityRange>(entity).expect("range")),
                bounds(&expected[level]),
                "level {level} must carry its adaptive threshold"
            );
        }
    }

    /// Engaged, the clamp pins every tile to ONE level: the requested rung shows over `[0, ∞)` and
    /// every other level of the tile is given a range no distance can satisfy. Returning to
    /// `Adaptive` restores the live thresholds — the clamp must not be a one-way door.
    #[test]
    fn the_rung_clamp_pins_one_level_and_releases_cleanly() {
        let (mut app, entities) = clamp_fixture();
        app.update();
        let ladder = synthetic_ladder();
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
        let view = terrain_view(*app.world().resource::<ViewFacts>());
        let expected = ladder.ranges(&ladder.tiles[0], view);
        for (level, &entity) in entities.iter().enumerate() {
            assert_eq!(
                bounds(app.world().get::<VisibilityRange>(entity).expect("range")),
                bounds(&expected[level]),
                "level {level} must be back on its adaptive threshold"
            );
        }
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

    /// A FOV OUTSIDE `(0, π)` cannot reach the range chain. It is a DIVISOR: `NaN` makes every
    /// boundary `NaN`, and a `NaN` boundary compares false against every distance — the ground stops
    /// being drawn at all, with no error anywhere. A negative one inverts the chain, and π or more
    /// has no perspective half-angle. `spec::TankSpec::validate` refuses to load such a sheet over
    /// the same interval; this is the second line, for a projection written by something that is not
    /// an authored view. The accepted end of the interval is checked too, so a fallback that
    /// swallowed every value would fail here rather than pass quietly.
    ///
    /// The guard itself lives in `crate::view` now, shared with the tank's chains — so what this
    /// pins is that the terrain RANGE CHAIN is behind it, which is the property that would be lost
    /// if this ladder ever grew a second way to build a profile.
    #[test]
    fn an_invalid_fov_cannot_reach_the_range_chain() {
        let lod = build(&shipped_grid());
        for fov in [
            0.0,
            -0.5,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            core::f32::consts::PI,
            6.0,
        ] {
            let view = terrain_view(ViewFacts::new(fov, 1440.0));
            assert_eq!(
                view.facts.vfov_rad,
                ViewFacts::default().vfov_rad,
                "fov {fov} must fall back"
            );
            for tile in &lod.tiles {
                for range in lod.ranges(tile, view) {
                    assert!(
                        range.start_margin.start.is_finite() && range.start_margin.start >= 0.0,
                        "fov {fov} produced boundary {}",
                        range.start_margin.start
                    );
                }
                // And the chain still covers everything exactly once.
                for distance in [0.0, 100.0, 1_000.0, 50_000.0] {
                    assert_eq!(
                        lod.ranges(tile, view)
                            .iter()
                            .filter(|range| range.is_visible_at_all(distance))
                            .count(),
                        1
                    );
                }
            }
        }
        // The legal interval is passed THROUGH, right up to its open ends.
        for fov in [1.0e-4, 0.12, std::f32::consts::FRAC_PI_4, 3.0] {
            assert_eq!(
                ViewFacts::new(fov, 1440.0).vfov_rad,
                fov,
                "fov {fov} is legal and must not be replaced"
            );
        }
        // Heights get the same treatment.
        for height in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                ViewFacts::new(0.5, height).height_px,
                ViewFacts::default().height_px,
                "height {height} must fall back"
            );
        }
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

    /// ONE PROJECTION, BOTH LADDERS. A deviation of a given size goes sub-pixel at one distance in
    /// this session, whether it is carried by a terrain rung or by a certified rung of the tank's
    /// geometry — the property two independent readers of the same camera could not have, and the
    /// one this ladder did not have while it divided by `fov` instead of by `2·tan(fov/2)`.
    ///
    /// The shortcut it replaced was conservative (it always returned the LARGER distance, holding
    /// the finer level nearer the eye), which is why nothing looked wrong; it also read 5.5 % long
    /// at the commander field, which is what ADR-0033 §9 refuses.
    #[test]
    fn the_terrain_ladder_derives_through_the_shared_exact_projection() {
        let commander = std::f32::consts::FRAC_PI_4;
        let radius = 133.4;
        let view = terrain_view(ViewFacts::new(commander, 2160.0));
        for (rung, &dev) in TERRAIN_LOD_LADDER.iter().enumerate().skip(1) {
            let wired = rung_switch_distance_m(view, rung, radius);
            // Same view, same deviation, same metres — reached through the CERTIFICATE's side of
            // the projection instead of the ladder's.
            let chain = crate::geometry_lod::Chain {
                radius_m: radius,
                rungs: vec![crate::geometry_lod::certificate::Rung {
                    mesh: "probe".to_owned(),
                    deviation_mm: dev * 1000.0,
                }],
            };
            assert_eq!(
                wired,
                chain.bands(view)[1].start_margin.start,
                "rung {rung} does not agree with the certificate's own derivation",
            );
            // And it is the exact form, never the shortcut.
            let exact = dev * 2160.0 / (2.0 * (commander / 2.0).tan() * TERRAIN_LOD_BUDGET_PX);
            assert!((wired - radius - exact).abs() < 1e-2, "rung {rung}");
            let small_angle = dev * 2160.0 / (commander * TERRAIN_LOD_BUDGET_PX);
            assert!(
                wired - radius < small_angle,
                "rung {rung} still reads the small-angle shortcut",
            );
        }
    }
}

/// THE TACTICAL REPORT: what a coarser level does to a SIGHTLINE, as opposed to what it does to a
/// silhouette.
///
/// The positional ladder is built on projected vertical error, and that metric structurally cannot
/// see motion ALONG a pixel ray. Near tangency the ray-surface intersection satisfies
/// `δt ≈ δh / (d_y − ∇h·d_xz)`, whose denominator goes to zero exactly where tanks look at each
/// other: a deviation invisible on screen can move a first-hit by hundreds of metres and decide
/// whether a hull sits behind a crest or on top of it.
///
/// # What this measures, and how it is kept honest
///
/// * **Exact intersection on both sides.** No marching. Both surfaces are piecewise linear, so a
///   ray meets them in closed form: the canonical side through `HeightGrid::cast_ray` (already
///   pinned against parry to 3.3e-4 m), the LOD side through [`Surface::cast`], a cell-walking
///   exact caster over the selected triangles, itself pinned against that same reference at level
///   zero and against an exhaustive ray/triangle sweep at a coarse level. Marching both surfaces at
///   a shared step would NOT do: two piecewise-linear surfaces can cross and re-separate between
///   the same pair of samples, so identical sampling aliases differently on each side, and the
///   difference of two aliased casts is not the difference of two surfaces.
/// * **The surface the observer would actually see.** [`Ladder::selected`] picks a level PER TILE
///   from the camera-to-AABB-centre distance and the live view profile — the same arithmetic
///   `check_visibility_ranges` performs — instead of capping the whole map at one rung. Near tiles
///   are exact while far tiles are coarse, which is the only configuration a player ever renders and
///   the only one in which a sightline crosses a switch.
/// * **Stratified, with coverage RECORDED.** Rays are drawn from a full-factorial grid of eye
///   height × pitch band × map region × azimuth octant, and the report states how many strata met
///   their quota and which rungs were actually exercised. A stratum short of quota is a thin corner
///   of the measurement and is stated rather than averaged away.
///
/// # IT MEASURES, IT DOES NOT GATE
///
/// Nothing tactical is asserted. The LOD doctrine has exactly one hard limit — worst-case surface
/// deviation projecting under the pixel budget through the live view — and grazing first-hit slide
/// and crest occlusion are among the channels known to escape it (module doc). They are REMEMBERED
/// rather than handled: the full table prints on every run so the numbers travel with the code and
/// a future reader sees the trend, but no threshold here pretends to own them. What is asserted is
/// that the report is worth reading — the caster is pinned against an independent oracle, the
/// coverage floor held, and at least one profile actually crossed a switch.
///
/// The sweep covers the whole view ENVELOPE rather than only the shipped 1 px budget, because
/// [`TERRAIN_LOD_BUDGET_PX`] is destined to become a player quality row sketched as wide as ~8 px
/// for low-end machines — an 8× cut in every switch distance, dragging coarse levels an order of
/// magnitude nearer the eye than the profile this ladder was designed around. A report that only
/// covered 1 px would go quiet exactly where the numbers get interesting.
///
/// It is also the only thing in this module that exercises the REAL selection path end to end:
/// level choice, AABB anchoring, the range chain, the compacted index buffers and the drawn
/// geometry all participate. That is the class that produced the two worst defects this module has
/// had — a distance measured to the world origin instead of the tile, and a harness that decoded
/// compacted indices as grid coordinates — and neither was visible to any test that checked
/// geometry in isolation.
#[cfg(test)]
mod tactical {
    use super::*;
    use crate::terrain_grid::tests::shipped_grid;

    /// One tile-level's geometry, indexed for O(1) point location.
    ///
    /// THE VERTICES ARE THE MESH'S OWN, verbatim, and the triangles index THOSE. A level's index
    /// buffer addresses its own compacted vertex buffer, in first-encounter order — it is not a
    /// patch-node index and carries no grid coordinate. Reconstructing a position from an index is
    /// therefore impossible, and attempting it silently yields a different surface from the one that
    /// renders, which is the whole failure mode this harness exists to avoid. Grid coordinates are
    /// recovered only from POSITIONS, and only to bucket triangles into cells.
    struct TileLevel {
        positions: Vec<Vec3>,
        tris: Vec<[u32; 3]>,
        /// The at-most-two triangles covering each of the tile's `MESH_TILE_CELLS²` cells. Two,
        /// because a cell centre can lie exactly ON a diagonal.
        cells: Vec<[u32; 2]>,
    }

    /// The whole ladder, indexed and ready for selection: `levels[tile][level]`.
    struct Ladder<'a> {
        grid: &'a HeightGrid,
        lod: TerrainLod,
        levels: Vec<Vec<TileLevel>>,
        tiles_per_side: usize,
    }

    impl<'a> Ladder<'a> {
        fn new(grid: &'a HeightGrid) -> Self {
            let lod = build(grid);
            let cells_per_side = grid.size() as usize - 1;
            let step = grid.world_size() / cells_per_side as f32;
            let mut levels = Vec::with_capacity(lod.tiles.len());
            for (tile, [ia, _, ja, _]) in lod.tiles.iter().zip(mesh_tile_node_ranges(grid)) {
                let mut per_level = Vec::with_capacity(tile.levels.len());
                for level in &tile.levels {
                    let (
                        Some(bevy::mesh::VertexAttributeValues::Float32x3(vertices)),
                        Some(Indices::U32(indices)),
                    ) = (
                        level.mesh.attribute(Mesh::ATTRIBUTE_POSITION),
                        level.mesh.indices(),
                    )
                    else {
                        panic!("a level must carry f32x3 positions and u32 indices");
                    };
                    let positions: Vec<Vec3> =
                        vertices.iter().copied().map(Vec3::from_array).collect();
                    let tris: Vec<[u32; 3]> = indices
                        .chunks_exact(3)
                        .map(|tri| [tri[0], tri[1], tri[2]])
                        .collect();
                    // Bucket each triangle into the unit cells it covers. The tile-local grid
                    // coordinate of a vertex is recovered from its POSITION — legitimate because
                    // every LOD vertex is an exact grid sample sitting exactly on a node
                    // (`interior_vertices_are_exact_grid_samples`), so the round-trip is exact.
                    let half = grid.half_extent();
                    let local = |p: Vec3| {
                        (
                            ((p.x + half) / step).round() as i64 - ia as i64,
                            ((p.z + half) / step).round() as i64 - ja as i64,
                        )
                    };
                    let mut cells = vec![[u32::MAX; 2]; MESH_TILE_CELLS * MESH_TILE_CELLS];
                    for (id, tri) in tris.iter().enumerate() {
                        let [a, b, c] = tri.map(|k| local(positions[k as usize]));
                        // Doubled coordinates: a cell centre is the integer point (2i+1, 2j+1), so
                        // the coverage test stays exact integer arithmetic.
                        let (ax, ay) = (a.0 * 2, a.1 * 2);
                        let (bx, by) = (b.0 * 2, b.1 * 2);
                        let (cx, cy) = (c.0 * 2, c.1 * 2);
                        let area2 = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
                        if area2 == 0 {
                            continue;
                        }
                        for j in a.1.min(b.1).min(c.1)..a.1.max(b.1).max(c.1) {
                            for i in a.0.min(b.0).min(c.0)..a.0.max(b.0).max(c.0) {
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
                                let slot = &mut cells[j as usize * MESH_TILE_CELLS + i as usize];
                                if slot[0] == u32::MAX {
                                    slot[0] = id as u32;
                                } else if slot[1] == u32::MAX {
                                    slot[1] = id as u32;
                                }
                            }
                        }
                    }
                    per_level.push(TileLevel {
                        positions,
                        tris,
                        cells,
                    });
                }
                levels.push(per_level);
            }
            let tiles_per_side = cells_per_side.div_ceil(MESH_TILE_CELLS);
            Self {
                grid,
                lod,
                levels,
                tiles_per_side,
            }
        }

        /// The tile containing world `(x, z)`.
        fn tile_at(&self, x: f32, z: f32) -> usize {
            let cells = self.grid.size() as usize - 1;
            let step = self.grid.world_size() / cells as f32;
            let half = self.grid.half_extent();
            let clamp = |w: f32| (((w + half) / step) as usize).min(cells - 1);
            (clamp(z) / MESH_TILE_CELLS) * self.tiles_per_side + clamp(x) / MESH_TILE_CELLS
        }

        /// The surface a camera at `eye` under view profile `view` would actually be shown: one
        /// level per tile, chosen by the same distance-to-AABB-centre rule
        /// `check_visibility_ranges` applies (`bevy_camera-0.19.0/src/visibility/range.rs:263`).
        fn selected(&self, eye: Vec3, view: ViewProfile) -> Surface<'_> {
            let choice = self
                .lod
                .tiles
                .iter()
                .map(|tile| {
                    let centre = (tile.min + tile.max) * 0.5;
                    let distance = (eye - centre).length();
                    self.lod
                        .ranges(tile, view)
                        .iter()
                        .position(|range| range.is_visible_at_all(distance))
                        // Past every end margin cannot happen while the chain reaches infinity;
                        // the coarsest level is the safe read if it ever does.
                        .unwrap_or(tile.levels.len() - 1)
                })
                .collect();
            Surface {
                ladder: self,
                choice,
            }
        }
    }

    /// One concrete drawn surface: the ladder plus a level choice per tile.
    struct Surface<'a> {
        ladder: &'a Ladder<'a>,
        choice: Vec<usize>,
    }

    impl Surface<'_> {
        /// EXACT first hit of `origin + t·dir` (unit `dir`) with the drawn surface, or `None`.
        ///
        /// A 2-D DDA walks the grid cells the ray's XZ footprint crosses, in ray order, and each
        /// cell's at-most-two triangles are solved in closed form. The walk keeps a running best and
        /// stops only once a cell's ENTRY parameter passes it — necessary because a coarse triangle
        /// registered in an early cell can be struck in a late one, so "first cell with a hit" is
        /// not "first hit".
        ///
        /// The world ends at the grid span, exactly as it does for the collider and
        /// `HeightGrid::cast_ray`.
        fn cast(&self, origin: Vec3, dir: Vec3, t_max: f32) -> Option<f32> {
            let ladder = self.ladder;
            let n = ladder.grid.size() as usize;
            let last = (n - 1) as f32;
            let (half, world) = (ladder.grid.half_extent(), ladder.grid.world_size());
            let (u0, v0) = (
                (origin.x + half) / world * last,
                (origin.z + half) / world * last,
            );
            let (du, dv) = (dir.x / world * last, dir.z / world * last);
            let (mut t0, mut t1) = (0.0f32, t_max);
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
            let cell = |w: f32| (w as usize).min(n - 2);
            let (mut i, mut j) = (cell(u0 + du * t0), cell(v0 + dv * t0));
            let (mut t_in, mut best) = (t0, None::<f32>);
            for _ in 0..2 * n {
                if best.is_some_and(|found| t_in > found) {
                    break;
                }
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
                if let Some(hit) = self.cell_hit(i, j, origin, dir, (t0, t1)) {
                    best = Some(best.map_or(hit, |found: f32| found.min(hit)));
                }
                if t_out >= t1 {
                    break;
                }
                if t_u <= t_out {
                    if du > 0.0 {
                        i += 1;
                    } else if i == 0 {
                        break;
                    } else {
                        i -= 1;
                    }
                }
                if t_v <= t_out {
                    if dv > 0.0 {
                        j += 1;
                    } else if j == 0 {
                        break;
                    } else {
                        j -= 1;
                    }
                }
                if i > n - 2 || j > n - 2 {
                    break;
                }
                t_in = t_out;
            }
            best
        }

        /// The nearest crossing of the triangles registered to grid cell `(i, j)`, within the hard
        /// clip `[t0, t1]`. Exact: each triangle's plane makes the crossing one division, and XZ
        /// barycentric containment decides whether that crossing lies ON the triangle. The surface
        /// is single-valued in XZ, so the projected test is the whole membership question.
        fn cell_hit(
            &self,
            i: usize,
            j: usize,
            origin: Vec3,
            dir: Vec3,
            (t0, t1): (f32, f32),
        ) -> Option<f32> {
            let ladder = self.ladder;
            let tile = (j / MESH_TILE_CELLS) * ladder.tiles_per_side + i / MESH_TILE_CELLS;
            let level = &ladder.levels[tile][self.choice[tile]];
            let local = (j % MESH_TILE_CELLS) * MESH_TILE_CELLS + (i % MESH_TILE_CELLS);
            let mut best: Option<f32> = None;
            for &id in &level.cells[local] {
                if id == u32::MAX {
                    continue;
                }
                let [a, b, c] = level.tris[id as usize].map(|k| level.positions[k as usize]);
                let normal = (b - a).cross(c - a);
                let denominator = normal.dot(dir);
                if denominator == 0.0 {
                    continue;
                }
                let t = normal.dot(a - origin) / denominator;
                if t < t0 || t > t1 || best.is_some_and(|found| t >= found) {
                    continue;
                }
                let p = origin + dir * t;
                let area = (b.x - a.x) * (c.z - a.z) - (b.z - a.z) * (c.x - a.x);
                if area == 0.0 {
                    continue;
                }
                let inv = 1.0 / area;
                let wa = ((c.x - b.x) * (p.z - b.z) - (c.z - b.z) * (p.x - b.x)) * inv;
                let wb = ((a.x - c.x) * (p.z - c.z) - (a.z - c.z) * (p.x - c.x)) * inv;
                let wc = ((b.x - a.x) * (p.z - a.z) - (b.z - a.z) * (p.x - a.x)) * inv;
                const EPS: f32 = -1.0e-5; // shared-edge ownership slop only
                if wa >= EPS && wb >= EPS && wc >= EPS {
                    best = Some(t);
                }
            }
            best
        }
    }

    /// A deterministic LCG stream (no platform RNG), the generator shape `terrain_grid`'s seeded
    /// pins use.
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

    /// Eye heights spanning the tank/optic envelope: hull roof, commander's head, tallest legal
    /// optic ride. Every engagement sightline is drawn from this band.
    const EYE_HEIGHTS_M: [f32; 3] = [1.5, 2.2, 3.0];
    /// Depression bands, radians. The first is the near-tangent case the whole harness exists for;
    /// the last is ordinary gunnery depression, present so the grazing numbers have a control.
    const PITCH_BANDS_RAD: [(f32, f32); 4] = [
        (0.0017, 0.0087),
        (0.0087, 0.0349),
        (0.0349, 0.0698),
        (0.0698, 0.1745),
    ];
    /// Map regions the observer is drawn from (2×2 quadrants).
    const REGIONS: usize = 4;
    /// Azimuth octants.
    const OCTANTS: usize = 8;
    /// PAIRED hits wanted per (eye × pitch band × region × octant) stratum.
    const PER_STRATUM: usize = 3;
    /// Draws allowed per stratum while chasing that quota. Bounded so a genuinely unreachable
    /// stratum (a grazing ray that leaves the map whatever the origin) still reports as empty
    /// instead of looping.
    const RETRY_BUDGET: usize = 40;
    /// Sightline range bands, metres — engagement ranges on a 1 km map.
    const RANGE_BANDS_M: [(f32, f32); 4] = [
        (200.0, 400.0),
        (400.0, 600.0),
        (600.0, 800.0),
        (800.0, 1000.0),
    ];

    /// The widest pixel budget the quality row has been sketched with, for low-end machines. Every
    /// switch distance scales as `1/budget` ([`ViewFacts::sub_pixel_distance_m`]), so this is the
    /// setting under
    /// which coarse levels sit nearest the camera and the reported numbers are worst.
    const WIDEST_SKETCHED_BUDGET_PX: f32 = 8.0;

    /// The view profiles the report covers: the two shipped fields at native 4K and one pixel, plus
    /// the commander field at [`WIDEST_SKETCHED_BUDGET_PX`]. The last is the binding case — widest
    /// field × loosest budget puts the coarsest geometry nearest the eye — and it is in the sweep so
    /// the table shows the whole envelope, not just the setting that ships today.
    fn reported_profiles() -> [(&'static str, ViewProfile); 3] {
        let commander = std::f32::consts::FRAC_PI_4;
        [
            (
                "gunner optic 4K @ 1 px",
                terrain_view(ViewFacts::new(crate::camera::GUNNER_FOV_FALLBACK, 2160.0)),
            ),
            (
                "commander 4K @ 1 px",
                terrain_view(ViewFacts::new(commander, 2160.0)),
            ),
            (
                "commander 4K @ 8 px (widest sketched quality setting)",
                ViewProfile::of(ViewFacts::new(commander, 2160.0), WIDEST_SKETCHED_BUDGET_PX),
            ),
        ]
    }

    /// THE TACTICAL REPORT. Prints, per view profile in [`reported_profiles`]: first-hit distance
    /// error against the exact ground, hit/miss disagreements, crest-occlusion flips split by
    /// direction, and the stratum coverage behind every one of those numbers.
    ///
    /// Every one of those is REPORTED. None is gated — see the module doc: sub-pixel projected
    /// deviation is the only hard limit in the doctrine, and these are the channels deliberately
    /// left un-special-cased. The assertions here are structural only: the coverage floor, every
    /// range band populated, and at least one profile having actually crossed a switch (without
    /// which the whole table could read as zeroes while measuring the exact surface against
    /// itself).
    ///
    /// Run it with `cargo test --lib tactical -- --nocapture`.
    #[test]
    fn tactical_ray_harness_reports() {
        let grid = shipped_grid();
        let ladder = Ladder::new(&grid);
        let radius = ladder.lod.bounding_radius_m;
        let strata = EYE_HEIGHTS_M.len() * PITCH_BANDS_RAD.len() * REGIONS * OCTANTS;
        println!(
            "TACTICAL REPORT (measured, not gated — see the module doc's doctrine note)\n  exact \
             ray/triangle on both surfaces; per-tile level selection from camera-to-AABB distance; \
             {strata} strata × {PER_STRATUM} paired hits"
        );
        let mut switched_somewhere = false;
        for (name, view) in reported_profiles() {
            println!(
                "\n  {name} — switch distances: {}",
                (1..TERRAIN_LOD_LADDER.len())
                    .map(|rung| format!(
                        "δ{:.2}@{:.0}m",
                        TERRAIN_LOD_LADDER[rung],
                        rung_switch_distance_m(view, rung, radius)
                    ))
                    .collect::<Vec<_>>()
                    .join(" ")
            );

            // ---- stratified grazing first-hit sweep -----------------------------------------
            let mut next = lcg(0x9E37_79B9_7F4A_7C15);
            let mut errors: Vec<f32> = Vec::new();
            let (mut paired, mut only_exact, mut only_lod, mut neither) = (0, 0, 0, 0);
            let mut filled = vec![false; strata];
            // Which rung the tile containing the exact first hit was SELECTED at — the column that
            // says whether the sweep crossed a switch at all, or merely re-measured level zero.
            let mut by_rung = vec![0usize; TERRAIN_LOD_LADDER.len()];
            let mut worst_by_rung = vec![0.0f32; TERRAIN_LOD_LADDER.len()];
            let mut stratum = 0usize;
            for eye in EYE_HEIGHTS_M {
                for (lo, hi) in PITCH_BANDS_RAD {
                    for region in 0..REGIONS {
                        for octant in 0..OCTANTS {
                            // Re-draw within the stratum until it yields its quota of PAIRED hits.
                            // A near-tangent ray can legitimately leave the map without ever meeting
                            // the ground, and letting that count as a filled stratum would hand back
                            // a coverage number that is really a miss-rate. The retry budget is
                            // bounded, so a genuinely unreachable stratum still reports as empty.
                            let mut wanted = PER_STRATUM;
                            for _ in 0..RETRY_BUDGET {
                                if wanted == 0 {
                                    break;
                                }
                                let half = grid.half_extent() * 0.95;
                                let (qx, qz) = (region % 2, region / 2);
                                let x = -half + (qx as f32 + next()) * half;
                                let z = -half + (qz as f32 + next()) * half;
                                let azimuth = (octant as f32 + next()) * std::f32::consts::TAU
                                    / OCTANTS as f32;
                                let pitch = lo + next() * (hi - lo);
                                let origin = Vec3::new(x, grid.height_at(x, z) + eye, z);
                                let dir = Vec3::new(
                                    pitch.cos() * azimuth.cos(),
                                    -pitch.sin(),
                                    pitch.cos() * azimuth.sin(),
                                )
                                .normalize();
                                let surface = ladder.selected(origin, view);
                                let exact = grid.cast_ray(origin, dir, 1500.0).map(|hit| hit.t);
                                let drawn = surface.cast(origin, dir, 1500.0);
                                match (exact, drawn) {
                                    (Some(a), Some(b)) => {
                                        paired += 1;
                                        wanted -= 1;
                                        // FILLED means the stratum met its whole quota, not that it
                                        // managed one hit. A stratum that yields a single ray is a
                                        // corner of the envelope this report barely touched, and
                                        // counting it as covered is how a coverage number becomes a
                                        // claim nobody checked.
                                        filled[stratum] = wanted == 0;
                                        let error = (a - b).abs();
                                        errors.push(error);
                                        let hit = origin + dir * a;
                                        let tile = ladder.tile_at(hit.x, hit.z);
                                        let rung = ladder.lod.tiles[tile].levels
                                            [surface.choice[tile]]
                                            .rung;
                                        by_rung[rung] += 1;
                                        worst_by_rung[rung] = worst_by_rung[rung].max(error);
                                    }
                                    (Some(_), None) => only_exact += 1,
                                    (None, Some(_)) => only_lod += 1,
                                    (None, None) => neither += 1,
                                }
                            }
                            stratum += 1;
                        }
                    }
                }
            }
            let (median, p95, max) = quantiles(&mut errors);
            let covered = filled.iter().filter(|filled| **filled).count();
            // Name the strata that came up short. One that yields fewer than its quota of paired
            // hits is a thin corner of the envelope — the numbers rest on less evidence there — and
            // that belongs in the output beside them, not averaged into a percentage.
            let holes: Vec<String> = filled
                .iter()
                .enumerate()
                .filter(|(_, hit)| !**hit)
                .map(|(index, _)| {
                    let octant = index % OCTANTS;
                    let region = index / OCTANTS % REGIONS;
                    let band = index / (OCTANTS * REGIONS) % PITCH_BANDS_RAD.len();
                    let eye = index / (OCTANTS * REGIONS * PITCH_BANDS_RAD.len());
                    format!(
                        "eye {:.1} m / pitch {:.2}–{:.2}° / quadrant {region} / octant {octant}",
                        EYE_HEIGHTS_M[eye],
                        PITCH_BANDS_RAD[band].0.to_degrees(),
                        PITCH_BANDS_RAD[band].1.to_degrees(),
                    )
                })
                .collect();
            println!(
                "    first-hit error over {paired} paired hits: median {median:.3} m, p95 \
                 {p95:.3} m, max {max:.3} m\n    disagreements: only-exact {only_exact}, only-LOD \
                 {only_lod} (both missed {neither})\n    coverage: {covered}/{strata} strata met \
                 their {PER_STRATUM}-hit quota{}\n    by SELECTED rung at the hit — {}",
                if holes.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n    SHORT OF QUOTA (a grazing ray from here mostly leaves the map \
                         whatever the origin): {}",
                        holes.join("; ")
                    )
                },
                by_rung
                    .iter()
                    .enumerate()
                    .filter(|(_, count)| **count > 0)
                    .map(|(rung, count)| format!(
                        "δ{:.2}: {count} hits, worst {:.3} m",
                        TERRAIN_LOD_LADDER[rung], worst_by_rung[rung]
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            );

            // ---- crest occlusion, stratified by range band and azimuth ----------------------
            const HULL_M: f32 = 1.8;
            let mut next = lcg(0xB5AD_4ECE_DA1C_E2A9);
            let (mut reveals, mut hides, mut tested, mut discarded) = (0, 0, 0, 0);
            let mut by_band = vec![0usize; RANGE_BANDS_M.len()];
            for (band, (lo, hi)) in RANGE_BANDS_M.into_iter().enumerate() {
                for octant in 0..OCTANTS {
                    // Same discipline as the ray sweep: re-draw the ORIGIN until the target at
                    // this band's range and this octant's bearing lands on the map. Simply
                    // discarding an off-map draw starves the long bands instead — MEASURED 38 of a
                    // wanted 256 in the 800–1000 m band, on a map whose diagonal is 1414 m — and a
                    // long-range crest flip is exactly the case this is here to catch.
                    let mut wanted = 32;
                    for _ in 0..RETRY_BUDGET * 8 {
                        if wanted == 0 {
                            break;
                        }
                        let half = grid.half_extent() * 0.98;
                        let x = (next() * 2.0 - 1.0) * half;
                        let z = (next() * 2.0 - 1.0) * half;
                        let azimuth =
                            (octant as f32 + next()) * std::f32::consts::TAU / OCTANTS as f32;
                        let range = lo + next() * (hi - lo);
                        let (tx, tz) = (x + range * azimuth.cos(), z + range * azimuth.sin());
                        if !grid.contains_xz(tx, tz) {
                            discarded += 1;
                            continue;
                        }
                        wanted -= 1;
                        let from = Vec3::new(x, grid.height_at(x, z) + 2.2, z);
                        let to = Vec3::new(tx, grid.height_at(tx, tz) + HULL_M, tz);
                        let span = to - from;
                        let distance = span.length();
                        let dir = span / distance;
                        let reach = distance - 0.5;
                        let surface = ladder.selected(from, view);
                        tested += 1;
                        by_band[band] += 1;
                        match (
                            grid.cast_ray(from, dir, reach).is_some(),
                            surface.cast(from, dir, reach).is_some(),
                        ) {
                            (true, false) => reveals += 1,
                            (false, true) => hides += 1,
                            _ => {}
                        }
                    }
                }
            }
            println!(
                "    crest occlusion over {tested} sightlines ({discarded} discarded off-map): \
                 {reveals} REVEAL a hull the exact ground hides, {hides} hide one it shows\n    \
                 coverage by range band: {}",
                RANGE_BANDS_M
                    .iter()
                    .zip(&by_band)
                    .map(|((lo, hi), count)| format!("{lo:.0}–{hi:.0} m: {count}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );

            let switched = by_rung.iter().skip(1).sum::<usize>() > 0;
            switched_somewhere |= switched;
            if !switched {
                println!(
                    "    NOTE: every hit landed on an exact-level tile. At this profile the \
                     nearest coarse switch is {:.0} m and the farthest a camera can be from any \
                     tile centre on a {} m map is ~{:.0} m — terrain LOD never fires in this \
                     view, so these zeroes are the honest answer and not a passing gate.",
                    rung_switch_distance_m(
                        view,
                        ladder.lod.tiles[0]
                            .levels
                            .get(1)
                            .map_or(TERRAIN_LOD_LADDER.len() - 1, |level| level.rung),
                        radius,
                    ),
                    ladder.grid.world_size(),
                    ladder.grid.world_size() * std::f32::consts::SQRT_2 / 2.0 + radius,
                );
            }

            // NOTHING TACTICAL IS ASSERTED. Crest flips and first-hit error are printed above and
            // gated nowhere: the LOD doctrine has exactly one hard limit, and it is sub-pixel
            // projected deviation (module doc). These are the known channels that escape that
            // bound, deliberately remembered rather than handled, so the numbers travel with every
            // run and a future reader can see the trend without a threshold pretending to own it.
            //
            // What IS asserted is that the report is worth reading: at least 95 % of strata at full
            // quota and every range band non-empty. 95 % and not 100 % because on a finite map a
            // few near-tangent corners have no ground left to hit whatever the origin, so a
            // total-coverage rule could only be met by widening the strata until the number looked
            // good. The strata short of quota are named above, and a COLLAPSE in coverage — a
            // caster regression, a ladder that stops producing geometry — still fails.
            assert!(
                covered * 100 >= strata * 95,
                "{name}: only {covered}/{strata} strata met their {PER_STRATUM}-hit quota — short: \
                 {}",
                holes.join("; ")
            );
            assert!(
                by_band.iter().all(|count| *count > 0),
                "{name}: a range band produced no sightline"
            );
        }
        // Across the profiles, at least one must actually cross a switch. Without this the harness
        // could report all-zero forever while measuring the exact surface against itself — which is
        // exactly how a broken ladder walks through a green suite.
        assert!(
            switched_somewhere,
            "no view profile exercised a coarse level; the harness is measuring nothing"
        );
    }

    /// The caster is pinned against the one the repo already trusts, at the ONE level where the two
    /// are supposed to describe the same surface: level zero everywhere is the canonical ground, so
    /// [`Surface::cast`] must agree with `HeightGrid::cast_ray` — itself pinned against parry to
    /// 3.3e-4 m over a seeded direction sweep.
    ///
    /// This pin alone is NOT sufficient and must never be treated as such: level zero is the one
    /// level whose index buffer happens to be row-major over the patch, so a caster that
    /// misinterprets a compacted index buffer passes here and reports fiction everywhere else.
    /// `the_tactical_caster_matches_brute_force_on_a_coarse_surface` is the pin that covers that
    /// class, and the two are only meaningful together.
    #[test]
    fn the_tactical_caster_agrees_with_the_pinned_terrain_caster() {
        let grid = shipped_grid();
        let ladder = Ladder::new(&grid);
        // Level zero everywhere: the drawn surface IS the canonical one, so any disagreement is the
        // caster's own and not the ladder's.
        let surface = Surface {
            ladder: &ladder,
            choice: vec![0; ladder.lod.tiles.len()],
        };
        let mut next = lcg(0x243F_6A88_85A3_08D3);
        let (mut worst, mut hits) = (0.0f32, 0u32);
        for _ in 0..512 {
            let x = (next() * 2.0 - 1.0) * grid.half_extent() * 0.95;
            let z = (next() * 2.0 - 1.0) * grid.half_extent() * 0.95;
            let origin = Vec3::new(x, grid.height_at(x, z) + 0.5 + next() * 30.0, z);
            // Steep to shallow-grazing: a shallow ray amplifies any surface mismatch by
            // 1/sin(elevation), so this is the harshest agreement the two casters face.
            let elevation = 0.03 + next() * 1.47;
            let azimuth = next() * std::f32::consts::TAU;
            let dir = Vec3::new(
                elevation.cos() * azimuth.cos(),
                -elevation.sin(),
                elevation.cos() * azimuth.sin(),
            )
            .normalize();
            match (
                grid.cast_ray(origin, dir, 1500.0).map(|hit| hit.t),
                surface.cast(origin, dir, 1500.0),
            ) {
                (Some(a), Some(b)) => {
                    hits += 1;
                    worst = worst.max((a - b).abs());
                }
                (None, None) => {}
                (a, b) => {
                    panic!(
                        "hit/no-hit disagreement at {origin:?} dir {dir:?}: {a:?} vs parry {b:?}"
                    )
                }
            }
        }
        assert!(hits > 300, "the sweep must exercise hits ({hits})");
        assert!(
            worst < 1.0e-2,
            "the tactical caster disagrees with the pinned one by {worst} m over {hits} hits"
        );
    }

    /// THE COARSE-LEVEL PIN, and the one that matters: on a surface built from the COARSEST level
    /// every tile keeps, [`Surface::cast`] must agree exactly with an exhaustive ray/triangle sweep
    /// over the same meshes' own vertex buffers.
    ///
    /// A coarse level's index buffer addresses its own compacted vertex buffer in first-encounter
    /// order — it carries no grid coordinate, and any attempt to recover one from it produces a
    /// different surface that still casts, still hits, and still reports plausible numbers. Nothing
    /// downstream can tell. Only an independent traversal of the same geometry can, so the oracle
    /// here shares no code with the cell index or the DDA: it looks at every triangle.
    ///
    /// Brute force over every selected triangle is why this uses few rays. It is exhaustive per ray,
    /// which is what makes it an oracle.
    #[test]
    fn the_tactical_caster_matches_brute_force_on_a_coarse_surface() {
        let grid = shipped_grid();
        let ladder = Ladder::new(&grid);
        let choice: Vec<usize> = ladder
            .lod
            .tiles
            .iter()
            .map(|tile| tile.levels.len() - 1)
            .collect();
        assert!(
            choice.iter().any(|level| *level > 0),
            "the map must offer a coarse level for this pin to mean anything"
        );
        let surface = Surface {
            ladder: &ladder,
            choice: choice.clone(),
        };
        // Exhaustive oracle: every triangle of every selected level, no spatial index, no walk.
        let brute = |origin: Vec3, dir: Vec3, t_max: f32| -> Option<f32> {
            let mut best: Option<f32> = None;
            for (tile, level) in choice.iter().enumerate() {
                let level = &ladder.levels[tile][*level];
                for tri in &level.tris {
                    let [a, b, c] = tri.map(|k| level.positions[k as usize]);
                    let normal = (b - a).cross(c - a);
                    let denominator = normal.dot(dir);
                    if denominator == 0.0 {
                        continue;
                    }
                    let t = normal.dot(a - origin) / denominator;
                    if t < 0.0 || t > t_max || best.is_some_and(|found| t >= found) {
                        continue;
                    }
                    let p = origin + dir * t;
                    let area = (b.x - a.x) * (c.z - a.z) - (b.z - a.z) * (c.x - a.x);
                    if area == 0.0 {
                        continue;
                    }
                    let inv = 1.0 / area;
                    let wa = ((c.x - b.x) * (p.z - b.z) - (c.z - b.z) * (p.x - b.x)) * inv;
                    let wb = ((a.x - c.x) * (p.z - c.z) - (a.z - c.z) * (p.x - c.x)) * inv;
                    let wc = ((b.x - a.x) * (p.z - a.z) - (b.z - a.z) * (p.x - a.x)) * inv;
                    const EPS: f32 = -1.0e-5;
                    if wa >= EPS && wb >= EPS && wc >= EPS {
                        best = Some(t);
                    }
                }
            }
            best
        };
        let mut next = lcg(0x452_821E_638D_0137);
        let (mut worst, mut hits, mut cast) = (0.0f32, 0u32, 0u32);
        for _ in 0..64 {
            let x = (next() * 2.0 - 1.0) * grid.half_extent() * 0.9;
            let z = (next() * 2.0 - 1.0) * grid.half_extent() * 0.9;
            let origin = Vec3::new(x, grid.height_at(x, z) + 1.5 + next() * 10.0, z);
            // Weighted toward the grazing band the harness reports on, where a wrong surface moves
            // the hit furthest and is therefore easiest to catch.
            let elevation = 0.002 + next() * next() * 1.2;
            let azimuth = next() * std::f32::consts::TAU;
            let dir = Vec3::new(
                elevation.cos() * azimuth.cos(),
                -elevation.sin(),
                elevation.cos() * azimuth.sin(),
            )
            .normalize();
            cast += 1;
            match (
                surface.cast(origin, dir, 1500.0),
                brute(origin, dir, 1500.0),
            ) {
                (Some(a), Some(b)) => {
                    hits += 1;
                    worst = worst.max((a - b).abs());
                }
                (None, None) => {}
                (a, b) => panic!(
                    "the indexed caster and the exhaustive sweep disagree on hit/no-hit at \
                     {origin:?} dir {dir:?}: {a:?} vs {b:?}"
                ),
            }
        }
        assert!(hits * 2 > cast, "the sweep must mostly hit ({hits}/{cast})");
        // Same arithmetic on the same vertices in a different order: agreement is exact up to the
        // f32 rounding of one dot product, not a tolerance on the surface.
        assert!(
            worst < 1.0e-3,
            "the indexed caster departs from the exhaustive sweep by {worst} m over {hits} hits"
        );
    }
}
