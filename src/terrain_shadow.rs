//! The terrain's sun shadow as a baked occlusion field (issue #170): generated once, when the
//! heightfield loads, from the canonical [`HeightGrid`] toward the fixed sun, and handed to the
//! vendored renderer as `bevy::pbr::HeightfieldShadow`. Every lit fragment samples it — ground,
//! tanks, trees and buildings — so a ridge shades what stands under it with no terrain caster in
//! the cascades. Dynamic casters keep drawing into the cascades; the field multiplies their result
//! (`vendor/bevy_pbr-0.19.1-scalar-math/src/render/shadows.wgsl`, `fetch_heightfield_shadow`).
//!
//! # The record
//!
//! One texel per grid node, `Rg32Float`: R the height (m) and G the horizontal distance (m) along
//! the sun of the OCCLUDER that node's ray toward the sun meets, or G < 0 for none. The ray is
//! walked over the canonical triangulated surface ([`HeightGrid::height_at`], the one surface the
//! collider and every LOD level are certified against) at its exact breakpoints, the crossings of
//! the cell edges and the cell diagonals: between two of them the surface along the ray is one
//! triangle's line, so the highest sun line over the ray lies on one of them.
//!
//! The occluder is the highest point of the ray's sun line (`height − distance · tan elevation`)
//! AFTER the line's first local minimum. Two consequences carry the shader:
//!
//! * a node in shadow records what shades it, and a lit node records the "near miss" — the ridge
//!   whose shadow ends short of it — so the four texels around a fragment extrapolate the SAME
//!   ridge's line to it, and a shadow's edge on the ground lands where that line meets the
//!   fragment, not on the texel grid;
//! * the run of falling line a lit node starts on (its own slope toward the sun) is never an
//!   occluder, so a receiver drawn a little below the canonical surface (a coarse LOD level, a
//!   track print's parallax landing) is not shaded by the ground it stands for.
//!
//! A lit node keeps its near miss down to a floor below it ([`near_miss_depth`] plus the node's
//! local relief): a receiver in one of its cells can stand that low, and every fragment that blends
//! the node's texel lies within one cell diagonal of it along the ray. So on ground within that
//! relief, all four texels around a fragment near a shadow's edge record the crest that casts it,
//! and their bilinear blend reproduces the crest's line at the fragment exactly, whatever the
//! crest's angle to the sun (bilinear weights reproduce a linear function only when no texel is
//! left out). A near miss under the floor is recorded as none; the floor is also where the walk
//! stops.
//!
//! # What it costs the picture
//!
//! The field's texel is the grid's: `world_size / (size − 1)`, DERIVED 1.465 m on the shipped
//! 1500 m map at 1025². A shadow edge ACROSS the sun (a straight crest's, at any angle to it) lands
//! where the occluder's line meets the receiver, at sub-texel precision. An edge ALONG the sun (a
//! hill's shadow's flank, or where two crests' shadows meet) is the
//! interpolation of neighbouring rays, one texel wide. The edge's softness is the sun's disc: a
//! receiver on the occluder's line is half lit, and the penumbra widens with the distance to the
//! occluder, so a long, shallow shadow from a far crest at a grazing sun is a soft one. The field
//! ends where the sun's cascades end and at the map's edge; beyond the span no receiver exists.

use std::time::Instant;

use bevy::asset::RenderAssetUsages;
use bevy::pbr::HeightfieldShadow;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::terrain_grid::{HeightGrid, node_world_coord};

/// The deepest a drawn receiver stands below the canonical surface, m: the coarsest terrain LOD
/// rung's certified deviation, which also covers a track print's deepest parallax landing (the
/// tuning panel's slider stops at 300 mm; the shipped grounds cap at 150 mm). A near miss further
/// below a node than this plus the node's relief reaches no receiver.
const RECEIVER_BELOW_SURFACE_M: f32 =
    crate::terrain_lod::TERRAIN_LOD_LADDER[crate::terrain_lod::TERRAIN_LOD_LADDER.len() - 1];

/// How far below a node on level ground its texel keeps a near miss, m (module doc): the deepest
/// receiver, plus the sun line's drop across one cell diagonal. DERIVED 1.13 m for the shipped
/// 1.465 m texel under the 17° sun.
pub(crate) fn near_miss_depth(texel_m: f32, tan_elevation: f32) -> f32 {
    RECEIVER_BELOW_SURFACE_M + std::f32::consts::SQRT_2 * texel_m * tan_elevation
}

/// The sun's angular radius, rad: CLAIMED 16′ (the sun's mean apparent radius). The penumbra is
/// the sun's own disc seen past the occluder's edge, so it widens with the occluder's distance:
/// DERIVED ±0.25 m of depth at 50 m under the 17° sun, ±0.04 m at 8 m.
const SUN_ANGULAR_RADIUS_RAD: f32 = 0.00465;

/// The least penumbra half-width, m, at an occluder's foot where the disc's own vanishes. ASSUMED:
/// a centimetre, far above the f32 rounding of an interpolated world position on a 1500 m map
/// (DERIVED ulp at 750 m: 6.1e-5 m).
const MIN_PENUMBRA_M: f32 = 0.01;

/// A baked field: `size × size` records, row-major from the grid's `(0, 0)` node.
pub(crate) struct OcclusionField {
    pub(crate) size: u32,
    /// World XZ of texel `(0, 0)`.
    pub(crate) origin: Vec2,
    /// World metres between neighbouring texels.
    pub(crate) texel_m: f32,
    /// The sun's rise per horizontal metre.
    pub(crate) tan_elevation: f32,
    /// `[occluder height m, occluder distance m]`; a negative distance records no occluder.
    pub(crate) records: Vec<[f32; 2]>,
}

impl OcclusionField {
    /// Height (m) of texel `k`'s occluder's sun line over the texel itself, if it records one.
    pub(crate) fn line_over(&self, k: usize) -> Option<f32> {
        let [height, distance] = self.records[k];
        (distance >= 0.0).then_some(height - distance * self.tan_elevation)
    }
}

/// The sun line one node's ray is walked against: its direction, slope and the map's walk limits.
struct Ray {
    /// Unit horizontal direction toward the sun.
    dir: Vec2,
    /// The sun's rise per horizontal metre.
    tan_elevation: f32,
    /// Metres along the ray between successive crossings of the cell edges along X, along Z, and
    /// of the cell diagonals; infinite for a family the ray runs parallel to.
    steps: [f32; 3],
    /// The highest sample on the map: no point further along than where its line drops under the
    /// best found so far can beat it.
    max_height: f32,
    /// [`near_miss_depth`] for this grid and sun.
    near_miss_depth: f32,
}

/// Bake `grid`'s occlusion toward `toward_sun` (a direction with a positive rise). Pure, and
/// deterministic for one grid and direction.
pub(crate) fn bake(grid: &HeightGrid, toward_sun: Vec3) -> OcclusionField {
    let n = grid.size() as usize;
    let texel_m = grid.world_size() / (n - 1) as f32;
    let horizontal = Vec2::new(toward_sun.x, toward_sun.z);
    let run = horizontal.length();
    assert!(
        run > 0.0 && toward_sun.y > 0.0,
        "the terrain shadow needs a sun above the horizon and off the zenith, got {toward_sun}"
    );
    let dir = horizontal / run;
    let step = |rate: f32| {
        if rate.abs() > f32::EPSILON {
            texel_m / rate.abs()
        } else {
            f32::INFINITY
        }
    };
    let ray = Ray {
        dir,
        tan_elevation: toward_sun.y / run,
        steps: [step(dir.x), step(dir.y), step(dir.x + dir.y)],
        max_height: (0..n)
            .flat_map(|j| (0..n).map(move |i| (i, j)))
            .map(|(i, j)| grid.sample(i, j))
            .fold(f32::NEG_INFINITY, f32::max),
        near_miss_depth: near_miss_depth(texel_m, toward_sun.y / run),
    };
    let mut records = vec![[0.0, -1.0]; n * n];
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(n);
    let rows_per_thread = n.div_ceil(threads);
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in records.chunks_mut(rows_per_thread * n).enumerate() {
            let ray = &ray;
            scope.spawn(move || {
                for (offset, record) in chunk.iter_mut().enumerate() {
                    let k = chunk_index * rows_per_thread * n + offset;
                    *record = record_at(grid, ray, k % n, k / n);
                }
            });
        }
    });
    OcclusionField {
        size: n as u32,
        origin: Vec2::splat(-grid.half_extent()),
        texel_m,
        tan_elevation: ray.tan_elevation,
        records,
    }
}

/// Node `(i, j)`'s occluder record (module doc).
fn record_at(grid: &HeightGrid, ray: &Ray, i: usize, j: usize) -> [f32; 2] {
    let (x0, z0) = (node_world_coord(grid, i), node_world_coord(grid, j));
    let h0 = grid.sample(i, j);
    let floor = h0 - local_relief(grid, i, j) - ray.near_miss_depth;
    // Crossing counts per family; the node itself lies on all three families' lines.
    let mut counts = [1u32; 3];
    let mut previous = h0;
    let mut rising = false;
    let mut best = f32::NEG_INFINITY;
    let mut best_t = -1.0;
    loop {
        let (family, t) = (0..3)
            .map(|family| (family, counts[family] as f32 * ray.steps[family]))
            .fold((0, f32::INFINITY), |a, b| if b.1 < a.1 { b } else { a });
        if !t.is_finite() || ray.max_height - t * ray.tan_elevation <= best.max(floor) {
            break;
        }
        let (x, z) = (x0 + t * ray.dir.x, z0 + t * ray.dir.y);
        if !grid.contains_xz(x, z) {
            break;
        }
        let line = grid.height_at(x, z) - t * ray.tan_elevation;
        rising |= line > previous;
        if rising && line > best {
            best = line;
            best_t = t;
        }
        previous = line;
        counts[family] += 1;
    }
    if best > floor {
        [best + best_t * ray.tan_elevation, best_t]
    } else {
        [0.0, -1.0]
    }
}

/// The largest height difference (m) between node `(i, j)` and its eight neighbours: how far below
/// the node a receiver in one of its cells can stand on the surface itself.
fn local_relief(grid: &HeightGrid, i: usize, j: usize) -> f32 {
    let n = grid.size() as usize;
    let h0 = grid.sample(i, j);
    let mut relief = 0.0f32;
    for nj in j.saturating_sub(1)..=(j + 1).min(n - 1) {
        for ni in i.saturating_sub(1)..=(i + 1).min(n - 1) {
            relief = relief.max((grid.sample(ni, nj) - h0).abs());
        }
    }
    relief
}

/// Bake the field toward `toward_sun` and hand it to the renderer. Called once, from the world's
/// spawn, on windowed compositions only.
pub(crate) fn install(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    grid: &HeightGrid,
    toward_sun: Vec3,
) {
    let started = Instant::now();
    let field = bake(grid, toward_sun);
    let n = field.size as usize;
    let shadowed = (0..field.records.len())
        .filter(|&k| {
            field
                .line_over(k)
                .is_some_and(|line| line > grid.sample(k % n, k / n))
        })
        .count();
    info!(
        "terrain shadow: {size}² field, {texel:.3} m texel, {shadowed} nodes in shadow, baked in \
         {ms:.0} ms",
        size = field.size,
        texel = field.texel_m,
        ms = started.elapsed().as_secs_f64() * 1000.0,
    );
    let image = Image::new(
        Extent3d {
            width: field.size,
            height: field.size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        texel_bytes(&field),
        TEXEL_FORMAT,
        RenderAssetUsages::RENDER_WORLD,
    );
    commands.insert_resource(handed_over(&field, images.add(image), toward_sun));
}

/// The texture format of the field's records (module doc).
const TEXEL_FORMAT: TextureFormat = TextureFormat::Rg32Float;

/// `field`'s records as [`TEXEL_FORMAT`] texels, row-major.
fn texel_bytes(field: &OcclusionField) -> Vec<u8> {
    field
        .records
        .iter()
        .flat_map(|[height, distance]| {
            height
                .to_le_bytes()
                .into_iter()
                .chain(distance.to_le_bytes())
        })
        .collect()
}

/// What the renderer is handed for `field` baked toward `toward_sun`, its records in `image`.
fn handed_over(
    field: &OcclusionField,
    image: Handle<Image>,
    toward_sun: Vec3,
) -> HeightfieldShadow {
    HeightfieldShadow {
        image,
        origin: field.origin,
        texel_m: field.texel_m,
        direction_to_light: toward_sun.normalize(),
        angular_radius: SUN_ANGULAR_RADIUS_RAD,
        min_penumbra_m: MIN_PENUMBRA_M,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain_grid::TerrainExtent;

    /// Fixture suns as (elevation°, azimuth° from +X toward +Z): low and high, on and off the
    /// cells' diagonals, so nothing below leans on one direction's alignment with the grid.
    const SUNS: [(f32, f32); 4] = [(17.0, 45.0), (17.0, 10.0), (30.0, 200.0), (12.0, 290.0)];

    fn sun(elevation_deg: f32, azimuth_deg: f32) -> Vec3 {
        let (sin_e, cos_e) = elevation_deg.to_radians().sin_cos();
        let (sin_a, cos_a) = azimuth_deg.to_radians().sin_cos();
        Vec3::new(cos_e * cos_a, sin_e, cos_e * sin_a)
    }

    /// A `size²` grid over `world_m`, its node heights `f(x, z)`.
    fn grid(size: u32, world_m: f32, f: impl Fn(f32, f32) -> f32) -> HeightGrid {
        let extent = TerrainExtent {
            world_size_m: world_m,
            height_offset_m: 0.0,
            height_span_m: 1.0,
        };
        let shell = HeightGrid::new(vec![0.0; (size * size) as usize].into(), size, extent);
        let n = size as usize;
        let samples: Vec<f32> = (0..n * n)
            .map(|k| {
                f(
                    node_world_coord(&shell, k % n),
                    node_world_coord(&shell, k / n),
                )
            })
            .collect();
        HeightGrid::new(samples.into(), size, extent)
    }

    /// Lit open ground records no occluder, so a receiver drawn below the canonical surface there
    /// (a coarse LOD level, a print's parallax landing) is never shaded by the ground it stands
    /// for. A mutant recording a node's own slope toward the sun fails here.
    #[test]
    fn open_ground_records_no_occluder() {
        for (elevation, azimuth) in SUNS {
            let toward = sun(elevation, azimuth);
            let dir = Vec2::new(toward.x, toward.z).normalize();
            // Half as steep as the sun's rays, rising toward it.
            let rise = 0.5 * toward.y / Vec2::new(toward.x, toward.z).length();
            for terrain in [
                grid(33, 64.0, |_, _| 5.0),
                grid(33, 64.0, |x, z| 20.0 + rise * Vec2::new(x, z).dot(dir)),
            ] {
                let field = bake(&terrain, toward);
                assert!(
                    (0..field.records.len()).all(|k| field.line_over(k).is_none()),
                    "sun ({elevation}°, {azimuth}°): open ground recorded an occluder"
                );
            }
        }
    }

    /// The walk visits the surface at its exact breakpoints, so a node's recorded sun line is the
    /// TRUE highest line over its ray: never below a dense sampling of the same canonical surface
    /// (`height_at`), above it by no more than that sampling can miss, and the field shades
    /// exactly the nodes the surface shades. Smooth fixture relief, generic suns.
    #[test]
    fn records_match_a_dense_walk_of_the_canonical_surface() {
        let terrain = grid(41, 60.0, |x, z| {
            6.0 + 3.0 * (0.21 * x).sin() * (0.17 * z).cos() + 2.0 * (0.09 * (x + 2.0 * z)).sin()
        });
        let n = terrain.size() as usize;
        // DERIVED: the fixture's sun line falls or climbs at under 1.4 m per metre (its steepest
        // gradient plus tan 30°), so a walk sampled every 5 mm misses the true maximum by < 7 mm.
        const STEP_M: f32 = 0.005;
        const TOLERANCE_M: f32 = 0.007;
        for (elevation, azimuth) in SUNS {
            let toward = sun(elevation, azimuth);
            let field = bake(&terrain, toward);
            let dir = Vec2::new(toward.x, toward.z).normalize();
            let mut shaded = 0;
            for k in (0..n * n).step_by(7) {
                let (i, j) = (k % n, k / n);
                let (x0, z0) = (node_world_coord(&terrain, i), node_world_coord(&terrain, j));
                let h0 = terrain.sample(i, j);
                let mut dense = f32::NEG_INFINITY;
                let mut t = STEP_M;
                while terrain.contains_xz(x0 + t * dir.x, z0 + t * dir.y) {
                    dense = dense.max(
                        terrain.height_at(x0 + t * dir.x, z0 + t * dir.y) - t * field.tan_elevation,
                    );
                    t += STEP_M;
                }
                let line = field.line_over(k);
                if dense > h0 + TOLERANCE_M {
                    shaded += 1;
                    let line = line.unwrap_or_else(|| {
                        panic!("node ({i}, {j}) is shaded but records no occluder")
                    });
                    assert!(
                        line >= dense - 1e-4 && line <= dense + TOLERANCE_M,
                        "node ({i}, {j}), sun ({elevation}°, {azimuth}°): recorded line {line} \
                         against the dense walk's {dense}"
                    );
                } else if dense < h0 - TOLERANCE_M {
                    assert!(
                        line.is_none_or(|line| line <= h0),
                        "node ({i}, {j}), sun ({elevation}°, {azimuth}°): lit, but recorded a \
                         line {line:?} over its height {h0}"
                    );
                }
            }
            assert!(shaded > 10, "the fixture must shade nodes, shaded {shaded}");
        }
    }

    /// FIXTURE: a ridge along the cells' anti-diagonals, so the canonical surface is exactly a
    /// profile of `i + j` (each triangle is linear in it) and its shadow's edge is closed-form:
    /// 1.5 m texels, a 4 m plateau from `i + j = 40` to `46`, flat ground at 0 elsewhere.
    const RIDGE_SIZE: u32 = 49;
    const RIDGE_TEXEL_M: f32 = 1.5;
    const RIDGE_M: f32 = 4.0;
    const RIDGE_FROM: f32 = 40.0;
    const RIDGE_TO: f32 = 46.0;
    /// Suns whose rays climb in `i + j`, toward the ridge's near face from the plain below.
    const RIDGE_SUNS: [(f32, f32); 3] = [(17.0, 45.0), (17.0, 10.0), (25.0, 75.0)];

    fn ridge() -> HeightGrid {
        let world = RIDGE_TEXEL_M * (RIDGE_SIZE - 1) as f32;
        let half = world / 2.0;
        grid(RIDGE_SIZE, world, |x, z| {
            let w = ((x + half) / RIDGE_TEXEL_M + (z + half) / RIDGE_TEXEL_M).round();
            if (RIDGE_FROM..=RIDGE_TO).contains(&w) {
                RIDGE_M
            } else {
                0.0
            }
        })
    }

    /// For one of [`RIDGE_SUNS`]: the index units (`i + j`) its ray climbs per metre, and the index
    /// where the plateau's near side's line meets the plain.
    fn ridge_edge(toward: Vec3) -> (f32, f32) {
        let horizontal = Vec2::new(toward.x, toward.z);
        let dir = horizontal.normalize();
        let climb = (dir.x + dir.y) / RIDGE_TEXEL_M;
        let tan_elevation = toward.y / horizontal.length();
        (climb, RIDGE_FROM - RIDGE_M / tan_elevation * climb)
    }

    /// A shadow's edge on the ground lands where the occluder's line meets it, not on the texel
    /// grid: every node near the edge, shaded or lit, records the SAME ridge, so each one's line
    /// carried along its ray puts the edge at the one true place. The shader's extrapolation is
    /// that carry. A mutant that records nothing for a lit node blurs the edge over a texel; one
    /// that records a node's own slope moves it.
    #[test]
    fn every_node_near_a_ridge_shadow_places_its_edge_exactly() {
        let terrain = ridge();
        let n = terrain.size() as usize;
        for (elevation, azimuth) in RIDGE_SUNS {
            let toward = sun(elevation, azimuth);
            let field = bake(&terrain, toward);
            let dir = Vec2::new(toward.x, toward.z).normalize();
            let (climb, edge) = ridge_edge(toward);
            // The lit nodes that keep their near miss (module doc), on the fixture's level plain.
            let depth = near_miss_depth(field.texel_m, field.tan_elevation);
            let reach = 0.9 * depth / field.tan_elevation * climb;
            let mut near = 0;
            for k in 0..n * n {
                let w = (k % n + k / n) as f32;
                if !(edge - reach..RIDGE_FROM).contains(&w) {
                    continue;
                }
                let (x0, z0) = (
                    node_world_coord(&terrain, k % n),
                    node_world_coord(&terrain, k / n),
                );
                let to_ridge_m = (RIDGE_FROM - w) / climb;
                if !terrain.contains_xz(x0 + to_ridge_m * dir.x, z0 + to_ridge_m * dir.y) {
                    continue;
                }
                let line = field.line_over(k).unwrap_or_else(|| {
                    panic!("node {k} at index {w}, edge at {edge}, records no occluder")
                });
                let placed = w - line / field.tan_elevation * climb;
                assert!(
                    (placed - edge).abs() / climb < 1e-3,
                    "sun ({elevation}°, {azimuth}°): node {k} places the edge at index {placed}, \
                     the surface at {edge}"
                );
                near += 1;
            }
            assert!(
                near > 20,
                "the fixture must exercise the edge, saw {near} nodes"
            );
        }
    }

    /// Texel `(i, j)` is node `(i, j)`: the shader's texel coordinates and the surface's node
    /// coordinates are one mapping.
    #[test]
    fn the_field_is_laid_on_the_grid_nodes() {
        let terrain = grid(17, 30.0, |_, _| 0.0);
        let field = bake(&terrain, sun(17.0, 45.0));
        assert_eq!(field.size, terrain.size());
        assert_eq!(field.records.len(), (field.size * field.size) as usize);
        for k in [0usize, 5, 16] {
            let at = field.origin + Vec2::splat(k as f32 * field.texel_m);
            assert!((at.x - node_world_coord(&terrain, k)).abs() < 1e-5);
        }
    }

    /// With the field baked, no terrain level casts into the cascades (the field carries the
    /// shadow) while every level still receives (tanks' and trees' shadows fall on it); the raster
    /// arm keeps them casting. Spawned through the world's own terrain path, resolved by the render
    /// policy, read off bevy's own markers.
    #[test]
    fn baked_terrain_leaves_the_cascades_caster_lists() {
        use bevy::ecs::system::RunSystemOnce;
        use bevy::light::{NotShadowCaster, NotShadowReceiver};

        use crate::render_policy::VisualScope;
        use crate::terrain_lod::TerrainLodEntity;

        let terrain = grid(17, 30.0, |x, z| (0.3 * x).sin() + (0.2 * z).cos());
        for (scope, casts) in [
            (VisualScope::BAKED_CASTER, false),
            (VisualScope::WORLD_SOLID, true),
        ] {
            let mut app = App::new();
            app.add_plugins(crate::render_policy::plugin)
                .init_resource::<Assets<Mesh>>();
            let terrain = terrain.clone();
            app.world_mut()
                .run_system_once(
                    move |mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>| {
                        crate::terrain_lod::spawn(
                            &mut commands,
                            &mut meshes,
                            &Handle::default(),
                            &terrain,
                            None,
                            scope,
                        );
                    },
                )
                .expect("the terrain spawn runs");
            app.update();
            let world = app.world_mut();
            let levels: Vec<(bool, bool)> = world
                .query_filtered::<(Has<NotShadowCaster>, Has<NotShadowReceiver>), With<TerrainLodEntity>>()
                .iter(world)
                .collect();
            assert!(!levels.is_empty(), "the terrain spawned no level");
            for (not_caster, not_receiver) in levels {
                assert_eq!(not_caster, !casts, "{scope:?}: a level's casting");
                assert!(!not_receiver, "{scope:?}: a level stopped receiving");
            }
        }
    }

    /// The vendored receiver law (`shadows.wgsl`'s `fetch_heightfield_shadow`) and the field's
    /// uniform type, lifted out of the shipped shader text with the view bindings swapped for test
    /// ones: the field's texture at binding 0, its uniform at 1, and the directional lights a
    /// private array pointing toward `lights`. The compute entry `check` writes the law's
    /// visibility at each query of binding 2 (xyz a world position, w a light's index) to
    /// binding 3.
    fn receiver_law_module(lights: &[Vec3]) -> String {
        let shadows = include_str!("../vendor/bevy_pbr-0.19.1-scalar-math/src/render/shadows.wgsl");
        let types =
            include_str!("../vendor/bevy_pbr-0.19.1-scalar-math/src/render/mesh_view_types.wgsl");
        let law = &shadows[shadows
            .find("const HEIGHTFIELD_SHADOW_SAME_LIGHT")
            .expect("the law's light test")
            ..shadows
                .find("fn cascade_debug_visualization")
                .expect("the function after the law")];
        let start = types
            .find("struct HeightfieldShadow {")
            .expect("the field's uniform type");
        let uniform = &types[start..start + types[start..].find("};").expect("its end") + 2];
        let count = lights.len();
        let lights = lights
            .iter()
            .map(|l| format!("Light(vec3<f32>({:?}, {:?}, {:?}))", l.x, l.y, l.z))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{uniform}
struct Light {{ direction_to_light: vec3<f32> }}
struct Lights {{ directional_lights: array<Light, {count}> }}
var<private> lights: Lights = Lights(array<Light, {count}>({lights}));
@group(0) @binding(0) var heightfield_shadow_texture: texture_2d<f32>;
@group(0) @binding(1) var<uniform> heightfield_shadow: HeightfieldShadow;
@group(0) @binding(2) var<storage, read> queries: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> visibility: array<f32>;
{law}
@compute @workgroup_size(64)
fn check(@builtin(global_invocation_id) id: vec3<u32>) {{
    if id.x < arrayLength(&queries) {{
        let query = queries[id.x];
        visibility[id.x] = fetch_heightfield_shadow(u32(query.w), query.xyz);
    }}
}}
",
            law = law.replace("view_bindings::", ""),
        )
    }

    /// The receiver law every lit mesh's pipeline compiles is valid WGSL on its own, so a broken
    /// edit fails here, in CI, rather than when a player's first lit mesh builds its pipeline.
    #[test]
    fn the_receiver_law_validates() {
        let source = receiver_law_module(&[Vec3::Y, Vec3::X]);
        let module = wgpu::naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|err| panic!("{}", err.emit_to_string(&source)));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("the receiver law validates");
    }

    /// Run the shipped receiver law on a headless compute device over `field` (baked toward
    /// `lights[0]`, handed over exactly as [`install`] hands it), one visibility per query.
    fn run_receiver_law(field: &OcclusionField, lights: &[Vec3], queries: &[Vec4]) -> Vec<f32> {
        use bevy::render::render_resource::encase;
        use bevy::tasks::block_on;
        use wgpu::util::DeviceExt;

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("a headless compute adapter");
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("a headless compute device");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("heightfield shadow receiver law"),
            source: wgpu::ShaderSource::Wgsl(receiver_law_module(lights).into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let storage = |read_only| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        // The vendored view layout's own entry: the records are not filterable.
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    1,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(2, storage(true)),
                entry(3, storage(false)),
            ],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: None,
                    bind_group_layouts: &[Some(&layout)],
                    immediate_size: 0,
                }),
            ),
            module: &module,
            entry_point: Some("check"),
            compilation_options: default(),
            cache: None,
        });
        let texture = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: field.size,
                    height: field.size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TEXEL_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &texel_bytes(field),
        );
        let mut uniform = encase::UniformBuffer::new(Vec::new());
        uniform
            .write(&bevy::pbr::GpuHeightfieldShadow::from_field(&handed_over(
                field,
                Handle::default(),
                lights[0],
            )))
            .expect("the field's uniform layout");
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: uniform.as_ref(),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let query_bytes: Vec<u8> = queries
            .iter()
            .flat_map(|q| q.to_array())
            .flat_map(f32::to_le_bytes)
            .collect();
        let query_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &query_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let size = 4 * queries.len() as u64;
        let result = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view = texture.create_view(&default());
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: query_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: result.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&default());
        {
            let mut pass = encoder.begin_compute_pass(&default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups((queries.len() as u32).div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&result, 0, &readback, 0, size);
        queue.submit([encoder.finish()]);
        let (send, receive) = std::sync::mpsc::channel();
        readback.map_async(wgpu::MapMode::Read, .., move |result| {
            send.send(result).expect("readback receiver");
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .expect("headless compute completes");
        receive.recv().unwrap().expect("readback maps");
        let bytes = readback.get_mapped_range(..);
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_le_bytes(b))
            .collect()
    }

    /// The receiver law, run on the GPU over the ridge fixture's baked field. A fragment is lit
    /// above its occluder's line and shaded below it, at its OWN height: a column standing on the
    /// plain in the ridge's shadow (a hull-down tank's side and deck) is dark below the line and in
    /// sun above it, half lit on it, and never darker higher up. A ground-only mask shades or
    /// lights the whole column at once and fails here. The shadow's edge on the ground lands where
    /// the line meets the plain, between texels and wherever the fragment sits in its cell: half
    /// a metre either side of it is fully lit or fully shaded. A law that drops the carry along
    /// the ray, or blends unrecorded texels in as lit, fails; so does a bake that leaves a lit
    /// texel next to the edge without its near miss, once the crest runs oblique to the sun. Beyond the field's span, and for any light but the one it was baked for, the
    /// field is silent.
    #[test]
    #[ignore = "manual headless GPU correctness check; requires a compute adapter, never a window"]
    fn a_receiver_is_lit_above_its_occluders_line_and_shaded_below() {
        let terrain = ridge();
        let other_light = Vec3::new(-0.3, 0.9, 0.3).normalize();
        // DERIVED slack for the f32 carry of a line over a few metres on a 72 m fixture.
        const SLACK_M: f32 = 0.005;
        for (elevation, azimuth) in RIDGE_SUNS {
            let toward = sun(elevation, azimuth);
            let field = bake(&terrain, toward);
            let tan = field.tan_elevation;
            let (climb, edge) = ridge_edge(toward);
            // World XZ of real-valued node coordinates.
            let at = |i: f32, j: f32| field.origin + Vec2::new(i, j) * field.texel_m;
            // The column: on the plain, off the texel grid, under a line 2 m over the ground.
            let to_ridge_m = (RIDGE_M - 2.0) / tan;
            let w = RIDGE_FROM - to_ridge_m * climb;
            assert!(w < RIDGE_FROM - 1.0, "the column stands on the plain");
            let column = at(0.5 * w + 0.31, 0.5 * w - 0.31);
            let line = RIDGE_M - to_ridge_m * tan;
            let half_width = SUN_ANGULAR_RADIUS_RAD * to_ridge_m * (1.0 + tan * tan);
            let heights: Vec<f32> = (0..=84).map(|k| k as f32 * 0.05 - 0.2).collect();
            let mut queries: Vec<Vec4> = heights
                .iter()
                .map(|&y| Vec4::new(column.x, y, column.y, 0.0))
                .collect();
            queries.push(Vec4::new(column.x, line, column.y, 0.0));
            // The ground edge, half a metre either side along the ray and on it, at places along
            // it that sit differently in their cells.
            const ALONG_EDGE: [f32; 5] = [-0.45, -0.2, 0.05, 0.17, 0.33];
            let edge_queries = queries.len();
            for along in ALONG_EDGE {
                for offset_m in [-0.5, 0.5, 0.0] {
                    let w = edge + offset_m * climb;
                    let p = at(0.5 * w + along, 0.5 * w - along);
                    queries.push(Vec4::new(p.x, 0.0, p.y, 0.0));
                }
            }
            // Silence: open plain far in front of the edge, below the ground beyond the span, and
            // the shaded column's foot under another light.
            let silent = queries.len();
            let open = at(0.5 * (edge - 8.0), 0.5 * (edge - 8.0));
            queries.push(Vec4::new(open.x, 0.0, open.y, 0.0));
            queries.push(Vec4::new(field.origin.x - 5.0, -10.0, column.y, 0.0));
            queries.push(Vec4::new(column.x, 0.0, column.y, 1.0));
            let seen = run_receiver_law(&field, &[toward, other_light], &queries);
            let sun_name = format!("sun ({elevation}°, {azimuth}°)");
            for (&y, &v) in heights.iter().zip(&seen) {
                if y < line - half_width - SLACK_M {
                    assert!(
                        v < 1e-3,
                        "{sun_name}: {y} m, under the line at {line} m: {v}"
                    );
                } else if y > line + half_width + SLACK_M {
                    assert!(
                        v > 1.0 - 1e-3,
                        "{sun_name}: {y} m, over the line at {line} m: {v}"
                    );
                }
            }
            assert!(
                seen[..heights.len()]
                    .windows(2)
                    .all(|p| p[1] >= p[0] - 1e-4),
                "{sun_name}: the column darkens with height: {:?}",
                &seen[..heights.len()]
            );
            assert!(
                (seen[heights.len()] - 0.5).abs() < 0.05,
                "{sun_name}: half lit on the line, saw {}",
                seen[heights.len()]
            );
            for (k, along) in ALONG_EDGE.into_iter().enumerate() {
                let [before, behind, on] = [0, 1, 2].map(|q| seen[edge_queries + 3 * k + q]);
                assert!(
                    before > 1.0 - 1e-3,
                    "{sun_name}, {along} along the edge: lit before it: {before}"
                );
                assert!(
                    behind < 1e-3,
                    "{sun_name}, {along} along the edge: shaded behind it: {behind}"
                );
                assert!(
                    (on - 0.5).abs() < 0.1,
                    "{sun_name}, {along} along the edge: half lit on it: {on}"
                );
            }
            assert_eq!(
                &seen[silent..],
                &[1.0; 3],
                "{sun_name}: the open plain, beyond the span, another light"
            );
        }
    }
}
