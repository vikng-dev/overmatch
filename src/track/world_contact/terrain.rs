//! One height-grid facet plane per force column; no terrain triangulation or polygon overlay.

use crate::terrain_grid::HeightGrid;
use crate::track::oracle::TerrainContact;
use bevy::math::{Dir3, Vec3};

pub(super) struct Plane {
    point: Vec3,
    normal: Dir3,
    half_extent: f32,
}

impl Plane {
    pub(super) fn sample(grid: &HeightGrid, at: Vec3) -> Option<Self> {
        if !grid.contains_xz(at.x, at.z) {
            return None; // The finite grid supplies no surface outside the map.
        }
        let last = (grid.size() - 1) as f32;
        let scale = last / grid.world_size();
        let u = (at.x + grid.half_extent()) / grid.world_size() * last;
        let v = (at.z + grid.half_extent()) / grid.world_size() * last;
        let i = (u as usize).min(grid.size() as usize - 2);
        let j = (v as usize).min(grid.size() as usize - 2);
        let h10 = grid.sample(i + 1, j);
        let h01 = grid.sample(i, j + 1);
        let (fu, fv) = (u - i as f32, v - j as f32);
        // The same anti-diagonal ownership as HeightGrid::height_at and the collider.
        let (dx, dz) = if fu + fv <= 1.0 {
            let h00 = grid.sample(i, j);
            ((h10 - h00) * scale, (h01 - h00) * scale)
        } else {
            let h11 = grid.sample(i + 1, j + 1);
            ((h11 - h01) * scale, (h11 - h10) * scale)
        };
        Some(Self {
            point: Vec3::new(at.x, grid.height_at(at.x, at.z), at.z),
            normal: Dir3::new(Vec3::new(-dx, 1.0, -dz))
                .expect("an admitted finite height facet has a nonzero upward normal"),
            half_extent: grid.half_extent(),
        })
    }

    pub(super) fn contact(&self, station: Vec3, out: Vec3, reach: f32) -> TerrainContact {
        let along = self.normal.dot(out);
        if along >= 0.0 {
            return TerrainContact::Miss; // A back-facing or grazing plane cannot be entered.
        }
        let depth = self.normal.dot(station - self.point) / along;
        let point = station - out * depth;
        if point.x.abs() > self.half_extent || point.z.abs() > self.half_extent {
            return TerrainContact::Miss; // Extrapolation never extends the finite map.
        }
        if depth >= reach {
            TerrainContact::Buried
        } else if depth < -reach {
            TerrainContact::Miss
        } else {
            TerrainContact::Hit {
                depth,
                normal: self.normal,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain_grid::TerrainExtent;
    use crate::track::oracle::{BlockField, ContactOracle, DepthOracle};
    use crate::track::world_contact::ColumnOracle;

    // Fixture: one nonplanar cell with distinct canonical facet normals.
    fn grid() -> HeightGrid {
        HeightGrid::new(
            vec![0.0, 0.0, 0.0, 1.0].into(),
            2,
            TerrainExtent {
                world_size_m: 4.0,
                height_offset_m: 0.0,
                height_span_m: 1.0,
            },
        )
    }

    /// Support inside one facet must use the collider's height, normal and directional depth.
    /// Both sides of the canonical diagonal are witnessed; bilinear normals fail this contract.
    #[test]
    fn a_sampled_facet_agrees_with_the_canonical_surface() {
        let grid = grid();
        for (x, z) in [(-1.0, -1.0), (1.0, 1.0)] {
            let at = Vec3::new(x, grid.height_at(x, z), z);
            let plane = Plane::sample(&grid, at).unwrap();
            for out in [Vec3::NEG_Y, Vec3::new(0.1, -1.0, 0.1).normalize()] {
                let station = at - Vec3::Y * 0.05; // Fixture: shallow penetration.
                let hit = grid.cast_ray(station - out * 0.5, out, 1.0).unwrap();
                let TerrainContact::Hit { depth, normal } = plane.contact(station, out, 0.5) else {
                    panic!("an exposed facet must carry the shallow contact");
                };
                assert!((depth - (0.5 - hit.t)).abs() < 1e-6);
                assert!((*normal - hit.normal).length() < 1e-6);
            }
        }
    }

    /// A quadrature plane supplies neither phantom ground outside the finite map nor a surface
    /// for an away-facing or buried probe. The force consumer relies on these distinct absences.
    #[test]
    fn finite_extent_and_first_surface_outcomes_are_preserved() {
        let grid = grid();
        assert!(Plane::sample(&grid, Vec3::new(2.1, 0.0, 0.0)).is_none());
        let plane = Plane::sample(&grid, Vec3::new(-1.0, 0.0, -1.0)).unwrap();
        assert!(matches!(
            plane.contact(Vec3::ZERO, Vec3::Y, 0.5),
            TerrainContact::Miss
        ));
        assert!(matches!(
            plane.contact(Vec3::NEG_Y, Vec3::NEG_Y, 0.5),
            TerrainContact::Buried
        ));
        assert!(matches!(
            plane.contact(Vec3::new(2.1, -0.1, 0.0), Vec3::NEG_Y, 0.5),
            TerrainContact::Miss
        ));
    }

    /// Work is one grid lookup per column, independent of how many depth and normal evaluations
    /// its integration needs. Stats must expose this path even when there are no solid candidates.
    #[test]
    fn column_integration_reuses_its_one_counted_grid_sample() {
        let field = BlockField::new(vec![]).with_height(Some(grid()));
        let stages = crate::cost::stages::Stages::default();
        let column = ColumnOracle::new(&field, Some(Vec3::new(-1.0, 0.0, -1.0)), Some(&stages));
        for y in [-0.01, -0.02, -0.03] {
            // Fixture: integration points in one facet.
            let p = Vec3::new(-1.0, y, -1.0);
            assert!((column.profile_depth_along(p, Vec3::NEG_Y, 0.5) + y).abs() < 1e-6);
            assert!(matches!(
                column.profile_contact_along(p, Vec3::NEG_Y, 0.5),
                TerrainContact::Hit { .. }
            ));
        }
        let counts = serde_json::to_value(stages.take_counts()).unwrap();
        assert_eq!(counts["belt_terrain_samples"], 1);
    }
}
