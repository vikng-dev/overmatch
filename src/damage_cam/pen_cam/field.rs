//! A spall cone's reach surface as a mesh: the surface that joins the points where each of the
//! record's directions was spent, closed back to the cone's origin along its rim. A shadowed
//! direction ends short, so a shadow shows as a dent from any angle.
//!
//! The record samples the cone at a few dozen directions. Their angular positions — polar angle
//! over the half-angle, azimuth about the axis — are triangulated (Delaunay), and each triangle is
//! subdivided with its corners' reach and strength interpolated linearly: a drawing between the
//! record's samples, never a direction the record does not hold at a corner. Positions are relative
//! to the cone's origin, so a copy scales about it and the stage's material holds a front, and
//! falls off, by distance from it.
//!
//! Vertex colour alpha carries the strength fraction `s(θ)/S` the record gave each direction; the
//! material turns it into brightness.
//!
//! Every triangle is wound outward (counter-clockwise seen from outside the closed surface): a
//! surface triangle so its corners' directions turn positively about the origin, which holds
//! however short their reaches, a rim triangle so its normal points away from the axis. So a
//! fragment's facing tells an eye ray entering the surface from one leaving it.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use super::plan::Cone;

/// Each Delaunay triangle is cut into `SUBDIVISION²` triangles.
const SUBDIVISION: usize = 4;

/// The reach surface of `cone` as a mesh about its origin.
pub fn reach_mesh(cone: &Cone) -> Mesh {
    let axis = cone.axis;
    let (u, v) = axis.any_orthonormal_pair();
    let angle = cone.half_angle.max(1e-3);
    let disc: Vec<Vec2> = cone
        .rays
        .iter()
        .map(|ray| {
            let theta = ray.direction.dot(axis).clamp(-1.0, 1.0).acos();
            let phi = ray.direction.dot(v).atan2(ray.direction.dot(u));
            Vec2::from_angle(phi) * (theta / angle)
        })
        .collect();
    let triangles = delaunay(&disc);
    // A disc point back to a direction: polar angle from the radius, azimuth from the angle.
    let direction = |q: Vec2| {
        let theta = q.length() * angle;
        let phi = q.y.atan2(q.x);
        (axis * theta.cos() + (u * phi.cos() + v * phi.sin()) * theta.sin()).normalize()
    };
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut directions: Vec<Vec3> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut vertex = |q: Vec2, reach: f32, weight: f32| {
        let along = direction(q);
        positions.push((along * reach).to_array());
        directions.push(along);
        colors.push([1.0, 1.0, 1.0, weight]);
        (positions.len() - 1) as u32
    };
    let n = SUBDIVISION;
    for [a, b, c] in &triangles {
        let corner = |i: usize| {
            let ray = &cone.rays[i];
            (disc[i], ray.reach, ray.weight)
        };
        let (ka, kb, kc) = (corner(*a), corner(*b), corner(*c));
        // Barycentric grid: row i from a toward c, column j toward b.
        let mut grid: Vec<Vec<u32>> = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let mut row = Vec::with_capacity(n + 1 - i);
            for j in 0..=(n - i) {
                let wc = i as f32 / n as f32;
                let wb = j as f32 / n as f32;
                let wa = 1.0 - wb - wc;
                let q = ka.0 * wa + kb.0 * wb + kc.0 * wc;
                let reach = ka.1 * wa + kb.1 * wb + kc.1 * wc;
                let weight = ka.2 * wa + kb.2 * wb + kc.2 * wc;
                row.push(vertex(q, reach, weight));
            }
            grid.push(row);
        }
        for i in 0..n {
            for j in 0..(n - i) {
                indices.extend([grid[i][j], grid[i][j + 1], grid[i + 1][j]]);
                if j + 1 < n - i {
                    indices.extend([grid[i][j + 1], grid[i + 1][j + 1], grid[i + 1][j]]);
                }
            }
        }
    }
    let surface = indices.len();
    // The rim: every hull edge closed back to the origin by a fan.
    for (a, b) in hull_edges(&triangles) {
        let origin = vertex(Vec2::ZERO, 0.0, 0.0);
        let mut previous = None;
        for k in 0..=n {
            let f = k as f32 / n as f32;
            let q = disc[a].lerp(disc[b], f);
            let reach = cone.rays[a].reach * (1.0 - f) + cone.rays[b].reach * f;
            let weight = cone.rays[a].weight * (1.0 - f) + cone.rays[b].weight * f;
            let current = vertex(q, reach, weight);
            if let Some(previous) = previous {
                indices.extend([origin, previous, current]);
            }
            previous = Some(current);
        }
    }
    for (at, triangle) in indices.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        let [a, b, c] = [0, 1, 2].map(|k| triangle[k] as usize);
        let inward = if 3 * at < surface {
            directions[a].dot(directions[b].cross(directions[c])) < 0.0
        } else {
            let point = |i: usize| Vec3::from(positions[i]);
            let normal = (point(b) - point(a)).cross(point(c) - point(a));
            let middle = (point(a) + point(b) + point(c)) / 3.0;
            normal.dot(middle - axis * middle.dot(axis)) < 0.0
        };
        if inward {
            triangle.swap(1, 2);
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh.duplicate_vertices();
    mesh.compute_flat_normals();
    mesh
}

/// Delaunay triangles of `points` (Bowyer–Watson), as index triples. Fewer than three points give
/// none.
pub fn delaunay(points: &[Vec2]) -> Vec<[usize; 3]> {
    if points.len() < 3 {
        return Vec::new();
    }
    let (lo, hi) = points.iter().fold((Vec2::MAX, Vec2::MIN), |(lo, hi), p| {
        (lo.min(*p), hi.max(*p))
    });
    let span = (hi - lo).max_element().max(1e-3) * 20.0;
    let mid = (lo + hi) * 0.5;
    let mut all: Vec<Vec2> = points.to_vec();
    let base = all.len();
    all.extend([
        mid + Vec2::new(-span, -span),
        mid + Vec2::new(span, -span),
        mid + Vec2::new(0.0, span),
    ]);
    let mut triangles: Vec<[usize; 3]> = vec![[base, base + 1, base + 2]];
    for (index, p) in points.iter().enumerate() {
        let (bad, good): (Vec<[usize; 3]>, Vec<[usize; 3]>) = triangles
            .into_iter()
            .partition(|t| in_circumcircle(&all, *t, *p));
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for t in &bad {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                if let Some(at) = edges
                    .iter()
                    .position(|&(x, y)| (x == b && y == a) || (x == a && y == b))
                {
                    edges.swap_remove(at);
                } else {
                    edges.push((a, b));
                }
            }
        }
        triangles = good;
        for (a, b) in edges {
            triangles.push(counter_clockwise(&all, [a, b, index]));
        }
    }
    triangles.retain(|t| t.iter().all(|&i| i < base));
    triangles
}

fn counter_clockwise(points: &[Vec2], [a, b, c]: [usize; 3]) -> [usize; 3] {
    let cross = (points[b] - points[a]).perp_dot(points[c] - points[a]);
    if cross < 0.0 { [a, c, b] } else { [a, b, c] }
}

fn in_circumcircle(points: &[Vec2], t: [usize; 3], p: Vec2) -> bool {
    let [a, b, c] = counter_clockwise(points, t).map(|i| points[i] - p);
    let det = (a.length_squared()) * b.perp_dot(c) - (b.length_squared()) * a.perp_dot(c)
        + (c.length_squared()) * a.perp_dot(b);
    det > 0.0
}

/// The edges that belong to one triangle only: the triangulation's boundary.
fn hull_edges(triangles: &[[usize; 3]]) -> Vec<(usize, usize)> {
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for t in triangles {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if let Some(at) = edges.iter().position(|&(x, y)| x == b && y == a) {
                edges.swap_remove(at);
            } else {
                edges.push((a, b));
            }
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::super::plan::Ray;
    use super::*;

    /// Golden-spiral points, as the cone's strata place its directions.
    fn spiral(n: usize) -> Vec<Vec2> {
        let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
        (0..n)
            .map(|k| {
                let r = ((k as f32 + 0.5) / n as f32).sqrt();
                Vec2::from_angle(k as f32 * golden) * r
            })
            .collect()
    }

    /// A planar triangulation of `n` points with `h` on its hull has `2n − h − 2` triangles, every
    /// one counter-clockwise, and no point inside any triangle's circumcircle.
    #[test]
    fn delaunay_triangulates_the_spiral() {
        for n in [3, 7, 24, 60] {
            let points = spiral(n);
            let triangles = delaunay(&points);
            let h = hull_edges(&triangles).len();
            assert_eq!(triangles.len(), 2 * n - h - 2, "n = {n}");
            for t in &triangles {
                assert_eq!(counter_clockwise(&points, *t), *t);
                for (i, p) in points.iter().enumerate() {
                    if !t.contains(&i) {
                        assert!(!in_circumcircle(&points, *t, *p), "n = {n}");
                    }
                }
            }
        }
    }

    /// A fixture cone of 24 spiral directions, direction `k` reaching `reach(k)`.
    fn fixture_cone(reach: impl Fn(usize) -> f32) -> Cone {
        let axis = Vec3::NEG_Z;
        let (u, v) = axis.any_orthonormal_pair();
        let half_angle = 25.0_f32.to_radians();
        let rays = spiral(24)
            .into_iter()
            .enumerate()
            .map(|(k, q)| {
                let theta = q.length() * half_angle;
                let phi = q.y.atan2(q.x);
                Ray {
                    direction: axis * theta.cos() + (u * phi.cos() + v * phi.sin()) * theta.sin(),
                    reach: reach(k),
                    weight: 1.0 - q.length(),
                }
            })
            .collect();
        Cone {
            origin: Vec3::ZERO,
            axis,
            half_angle,
            strength: 1.0,
            arc: 0.0,
            rays,
        }
    }

    /// The surface is closed and wound outward, however short its reaches: every edge is crossed
    /// once each way, and the volume it encloses, summed from its windings, is positive. Fixture
    /// reaches include directions stopped at the origin.
    #[test]
    fn the_surface_is_closed_and_wound_outward() {
        for reach in [
            Box::new(|k: usize| 1.0 + 0.1 * k as f32) as Box<dyn Fn(usize) -> f32>,
            Box::new(|k: usize| {
                if k.is_multiple_of(3) {
                    0.0
                } else {
                    0.5 + 0.2 * (k % 5) as f32
                }
            }),
        ] {
            let mesh = reach_mesh(&fixture_cone(reach));
            let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("positions");
            };
            let key = |p: [f32; 3]| p.map(|x| (x * 1.0e4).round() as i64);
            let mut edges = std::collections::HashMap::<([i64; 3], [i64; 3]), i32>::new();
            let mut volume = 0.0;
            for triangle in positions.as_chunks::<3>().0 {
                let [a, b, c] = [0, 1, 2].map(|k| Vec3::from(triangle[k]));
                volume += a.dot(b.cross(c)) / 6.0;
                for (from, to) in [(0, 1), (1, 2), (2, 0)] {
                    let (from, to) = (key(triangle[from]), key(triangle[to]));
                    if from != to {
                        *edges.entry((from, to)).or_default() += 1;
                        *edges.entry((to, from)).or_default() -= 1;
                    }
                }
            }
            assert!(volume > 0.0, "{volume}");
            assert!(
                edges.values().all(|n| *n == 0),
                "an edge crossed one way only"
            );
        }
    }

    /// The surface passes through every recorded reach end.
    #[test]
    fn the_surface_holds_every_reach_end() {
        let axis = Vec3::NEG_Z;
        let (u, v) = axis.any_orthonormal_pair();
        let half_angle = 25.0_f32.to_radians();
        let rays: Vec<Ray> = spiral(24)
            .into_iter()
            .enumerate()
            .map(|(k, q)| {
                let theta = q.length() * half_angle;
                let phi = q.y.atan2(q.x);
                let direction = axis * theta.cos() + (u * phi.cos() + v * phi.sin()) * theta.sin();
                Ray {
                    direction,
                    reach: 1.0 + 0.1 * k as f32,
                    weight: 1.0 - q.length(),
                }
            })
            .collect();
        let ends: Vec<Vec3> = rays.iter().map(|r| r.direction * r.reach).collect();
        let cone = Cone {
            origin: Vec3::ZERO,
            axis,
            half_angle,
            strength: 1.0,
            arc: 0.0,
            rays,
        };
        let mesh = reach_mesh(&cone);
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions");
        };
        for end in ends {
            assert!(
                positions
                    .iter()
                    .any(|p| Vec3::from(*p).distance(end) < 1e-4),
                "{end}"
            );
        }
    }
}
