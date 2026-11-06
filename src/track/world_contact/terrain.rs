//! Canonical grid facets, clipped in the same footprint frame as convex entry faces.

use super::convex::{ClippedFace, clip_faces};
use crate::terrain_grid::{HeightGrid, node_world_coord};
use crate::track::rubble::Slab;
use bevy::prelude::*;

pub(super) fn faces(grid: &HeightGrid, slab: &Slab) -> Vec<ClippedFace> {
    let (lo, hi) = slab.aabb();
    let half = grid.half_extent();
    if hi.x <= -half || lo.x >= half || hi.z <= -half || lo.z >= half {
        return Vec::new(); // A grid has no surface beyond its finite map boundary.
    }
    let last = grid.size() as usize - 2;
    let step = grid.world_size() / (grid.size() - 1) as f32;
    let cell = |x: f32| (((x + half) / step).floor().max(0.0) as usize).min(last);
    let mut triangles = Vec::new();
    for j in cell(lo.z)..=cell(hi.z) {
        for i in cell(lo.x)..=cell(hi.x) {
            let point = |i, j| {
                Vec3::new(
                    node_world_coord(grid, i),
                    grid.sample(i, j),
                    node_world_coord(grid, j),
                )
            };
            let (a, b, c, d) = (
                point(i, j),
                point(i + 1, j),
                point(i, j + 1),
                point(i + 1, j + 1),
            );
            // Parry's non-zigzag anti-diagonal, wound toward the heightfield's exterior (+Y).
            triangles.extend([[a, c, b], [b, c, d]]);
        }
    }
    clip_faces(&triangles, slab)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain_grid::TerrainExtent;
    use crate::track::forces::ForceParams;
    use crate::track::oracle::{BlockField, DepthOracle};
    use crate::track::rubble::RubbleBodies;
    use crate::track::world_contact::{PatchQuery, patches, sample};

    // Fixture: finite flat ground; tessellation cannot change its surface.
    fn flat_grid(size: u32) -> HeightGrid {
        HeightGrid::new(
            vec![0.0; (size * size) as usize].into(),
            size,
            TerrainExtent {
                world_size_m: 4.0,
                height_offset_m: 0.0,
                height_span_m: 1.0,
            },
        )
    }

    fn slab(x: f32) -> Slab {
        Slab {
            origin: Vec3::new(x, -0.1, -1.5),
            axis: Vec3::Z,
            lat: Vec3::X,
            out: Vec3::NEG_Y,
            len: 3.0,
            lateral: (-0.25, 0.25),
            face: 0.0,
            reach: 1.0,
            travel: 0.0,
        }
    }

    // Resolve through the world interface, including the force consumer's sample.
    fn moments(grid: HeightGrid, slab: &Slab) -> (f32, f32, Vec3) {
        let field = BlockField::new(vec![]).with_height(Some(grid.clone()));
        let pieces = RubbleBodies::default();
        let params = ForceParams {
            face_offset: slab.face,
            free_travel: 0.0,
            support_stiffness_per_m: 1.0,
            support_damping_per_m: 1.0,
            engage_depth: 0.001,
            probe_reach: slab.reach,
            grip: None,
        };
        let travel = [0.0; 3];
        let pen = [0.0, slab.len / 2.0, slab.len]
            .map(|u| field.profile_depth_along(slab.origin + slab.axis * u, slab.out, slab.reach));
        let patches = patches(
            PatchQuery {
                slab,
                overlaps: &[],
                strip: slab.lateral,
                field_offset: 0.0,
                weight: 1.0,
                pen,
                travel,
            },
            &params,
            &field,
            &pieces,
        );
        let mut pressure = 0.0;
        let mut length = 0.0;
        let mut moment = Vec3::ZERO;
        for patch in patches {
            let Some((a, l, c)) = patch.resultant() else {
                continue; // An uncovered footprint has no pressure resultant.
            };
            let surface = sample(&patch, slab.axis, slab.out, c, &params, &field, &pieces).unwrap();
            pressure += a;
            length += l;
            moment += surface.point * a;
            assert!(
                (surface.point.y - grid.height_at(surface.point.x, surface.point.z)).abs() < 1e-6
            );
            let hit = grid
                .cast_ray(surface.point + Vec3::Y, Vec3::NEG_Y, 2.0)
                .unwrap();
            assert!((surface.normal - hit.normal).length() < 1e-6);
        }
        (pressure, length, moment)
    }

    /// Driving must neither multiply pressure at internal edges nor lose the pressure lever arm.
    /// Fixture: a fully covered flat pad crosses grid diagonals and several cell borders.
    #[test]
    fn internal_edges_preserve_pressure_coverage_and_moment() {
        let slab = slab(0.0);
        for size in [2, 3, 5, 9] {
            let (p, l, m) = moments(flat_grid(size), &slab);
            // DERIVED: constant penetration 0.1 m over 3 m, symmetric footprint at y = 0.
            assert!((p - 0.3).abs() < 1e-6, "{size}: pressure {p}");
            assert!((l - 3.0).abs() < 1e-6, "{size}: length {l}");
            assert!(m.length() < 1e-6, "{size}: moment {m}");
        }
    }

    /// Contact must retain the canonical diagonal and each facet's own normal at a crease.
    /// Fixture: one raised corner in a single cell; bilinear or opposite-diagonal geometry
    /// changes both the pressure integral and its moment, which driving uses as a lever arm.
    #[test]
    fn a_nonplanar_cell_integrates_its_canonical_facets() {
        let grid = HeightGrid::new(
            vec![0.0, 0.0, 0.0, 1.0].into(),
            2,
            TerrainExtent {
                world_size_m: 4.0,
                height_offset_m: 0.0,
                height_span_m: 1.0,
            },
        );
        let slab = Slab {
            origin: Vec3::new(0.0, -0.1, -2.0),
            len: 4.0,
            lateral: (-2.0, 2.0),
            reach: 2.0,
            ..slab(0.0)
        };
        let (p, l, m) = moments(grid, &slab);
        // DERIVED: lower facet h=0, upper h=(x+z)/4; integrate (h+0.1)/width over
        // the square. Barycentric moments give pressure 16/15 and moment (2/3, 2/5, 2/3).
        assert!((p - 16.0 / 15.0).abs() < 1e-6, "pressure {p}");
        assert!((l - 4.0).abs() < 1e-6);
        assert!(
            (m - Vec3::new(2.0 / 3.0, 2.0 / 5.0, 2.0 / 3.0)).length() < 1e-6,
            "moment {m}"
        );
    }

    /// The world's finite boundary clips coverage, with no clamped phantom ground or edge shove.
    /// Fixture: half of the flat shoe width lies outside the grid, then the whole shoe does.
    #[test]
    fn map_edges_clip_area_without_creating_vertical_faces() {
        for x in [-2.0, 2.0] {
            let (p, l, m) = moments(flat_grid(5), &slab(x));
            assert!((p - 0.15).abs() < 1e-6);
            assert!((l - 1.5).abs() < 1e-6);
            // DERIVED: centroid of the covered half-width is 0.125 m inside the map.
            assert!((m.x / p - (x - x.signum() * 0.125)).abs() < 1e-6);
        }
        assert_eq!(moments(flat_grid(5), &slab(2.5)), (0.0, 0.0, Vec3::ZERO));
    }
}
