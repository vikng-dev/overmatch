//! A spall cone's recorded directions as one function of direction, and the surfaces drawn from it.
//!
//! The record samples the cone at its directions. Each direction's place on the cone's polar disc
//! — `q = (cos φ, sin φ) · θ/α`, polar angle θ over the half-angle α, azimuth φ about the axis —
//! is triangulated (Delaunay), and the reach and strength the record gives each are interpolated
//! linearly within the triangles: a drawing between the record's samples, never a direction the
//! record does not hold at a corner. [`Disc`] bakes that function onto a grid and reads it
//! bilinearly; the look's footprints read the same grid the same way (`pen_cam_look.wgsl`,
//! `disc_at`), and every band shell ([`shell_mesh`]) is drawn through its samples, so the shells
//! and the footprints draw one function of direction.
//!
//! A band shell joins, along every direction, the point out to which the field lies in that potency
//! band or a brighter one ([`Potency::shell_radius`]), closed back to the origin along the cone's
//! rim; the dim band's is the reach surface. A shadowed direction ends short, so a shadow shows as
//! a dent in every shell its reach cuts. The slice still draws the reach surface through the
//! record's own triangulation ([`reach_mesh`]). Positions are relative to the cone's origin, so
//! the look's material holds a front by distance from it.
//!
//! Every triangle is wound outward (counter-clockwise seen from outside the closed surface): a
//! surface triangle so its corners' directions turn positively about the origin, which holds
//! however short their reaches, a rim triangle so its normal points away from the axis. So a
//! fragment's facing tells an eye ray entering the surface from one leaving it.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use super::plan::{Cone, Ray};
use super::potency::{Band, Potency};

/// Each Delaunay triangle is cut into `SUBDIVISION²` triangles.
const SUBDIVISION: usize = 4;
/// The disc's grid: this many nodes a side over `q ∈ [−1, 1]²`, a node every 0.087 of the disc's
/// radius (DERIVED), against ~0.22 between the shipped cone's 64 directions (DERIVED: `√(π / 64)`).
/// Even, so the look packs two nodes a lane; `pen_cam_look.wgsl` declares the same side.
pub const DISC_SIDE: usize = 24;
/// A band shell's rings out from the axis and sectors about it: one a disc node's spacing apart
/// (DERIVED from [`DISC_SIDE`]), so the shell follows every node the footprint reads.
const SHELL_RINGS: usize = 12;
const SHELL_SECTORS: usize = 64;

/// A cone's recorded directions as one function over its polar disc: the disc's frame (the
/// cone's unit axis, its half-angle, and `u`, `v` across the axis, azimuth from `u` toward `v`)
/// and [`DISC_SIDE`]² nodes over `q ∈ [−1, 1]²`, row by row from `q.y = −1`, each the
/// `(reach m, strength ref-mm)` the triangulation of the record's directions interpolates there; a
/// node outside it takes the value at the nearest point of its boundary.
#[derive(Clone, Debug)]
pub struct Disc {
    pub axis: Vec3,
    pub half_angle: f32,
    pub u: Vec3,
    pub v: Vec3,
    pub nodes: Vec<Vec2>,
}

impl Disc {
    /// The disc of a cone about unit `axis` of half-angle `half_angle` (rad) whose on-axis strength
    /// is `strength` (ref-mm), sampled at `rays`.
    pub fn of(axis: Vec3, half_angle: f32, strength: f32, rays: &[Ray]) -> Self {
        let (u, v) = axis.any_orthonormal_pair();
        let mut disc = Self {
            axis,
            half_angle,
            u,
            v,
            nodes: Vec::new(),
        };
        let points: Vec<Vec2> = rays.iter().map(|ray| disc.point(ray.direction)).collect();
        let values: Vec<Vec2> = rays
            .iter()
            .map(|ray| Vec2::new(ray.reach, ray.weight * strength))
            .collect();
        let triangles = delaunay(&points);
        let boundary = hull_edges(&triangles);
        let step = 2.0 / (DISC_SIDE - 1) as f32;
        disc.nodes = (0..DISC_SIDE * DISC_SIDE)
            .map(|k| {
                let q = Vec2::new((k % DISC_SIDE) as f32, (k / DISC_SIDE) as f32) * step - 1.0;
                interpolate(&points, &values, &triangles, &boundary, q)
            })
            .collect();
        disc
    }

    /// The disc point of unit `direction`: `(cos φ, sin φ) · θ/α`.
    pub fn point(&self, direction: Vec3) -> Vec2 {
        let theta = direction.dot(self.axis).clamp(-1.0, 1.0).acos();
        let across = Vec2::new(direction.dot(self.u), direction.dot(self.v));
        across.normalize_or_zero() * (theta / self.half_angle.max(1e-3))
    }

    /// The unit direction at disc point `q`.
    pub fn direction(&self, q: Vec2) -> Vec3 {
        let theta = q.length() * self.half_angle.max(1e-3);
        let phi = q.y.atan2(q.x);
        let across = self.u * phi.cos() + self.v * phi.sin();
        (self.axis * theta.cos() + across * theta.sin()).normalize()
    }

    /// The `(reach m, strength ref-mm)` at disc point `q`, bilinear between its four nodes, `q`
    /// clamped to the grid.
    pub fn sample(&self, q: Vec2) -> Vec2 {
        let top = (DISC_SIDE - 1) as f32;
        let g = ((q + 1.0) * 0.5 * top).clamp(Vec2::ZERO, Vec2::splat(top));
        let low = g.floor().min(Vec2::splat(top - 1.0));
        let t = g - low;
        let (i, j) = (low.x as usize, low.y as usize);
        let at = |i: usize, j: usize| self.nodes[j * DISC_SIDE + i];
        let below = at(i, j).lerp(at(i + 1, j), t.x);
        let above = at(i, j + 1).lerp(at(i + 1, j + 1), t.x);
        below.lerp(above, t.y)
    }
}

/// The value the triangulation of `points` (carrying `values`) interpolates at `q`: barycentric
/// within the triangle that holds it, else at the nearest point of the `boundary` edges, else (no
/// triangle) the nearest point's; zero with no points.
fn interpolate(
    points: &[Vec2],
    values: &[Vec2],
    triangles: &[[usize; 3]],
    boundary: &[(usize, usize)],
    q: Vec2,
) -> Vec2 {
    const INSIDE: f32 = -1e-5;
    for &[a, b, c] in triangles {
        let (pa, pb, pc) = (points[a], points[b], points[c]);
        let area = (pb - pa).perp_dot(pc - pa);
        if area.abs() < 1e-12 {
            continue;
        }
        let wb = (q - pa).perp_dot(pc - pa) / area;
        let wc = (pb - pa).perp_dot(q - pa) / area;
        let wa = 1.0 - wb - wc;
        if wa >= INSIDE && wb >= INSIDE && wc >= INSIDE {
            return values[a] * wa + values[b] * wb + values[c] * wc;
        }
    }
    let mut nearest = (f32::INFINITY, Vec2::ZERO);
    for &(a, b) in boundary {
        let span = points[b] - points[a];
        let t = ((q - points[a]).dot(span) / span.length_squared().max(1e-12)).clamp(0.0, 1.0);
        let distance = q.distance(points[a] + span * t);
        if distance < nearest.0 {
            nearest = (distance, values[a].lerp(values[b], t));
        }
    }
    if boundary.is_empty() {
        for (point, value) in points.iter().zip(values) {
            let distance = q.distance(*point);
            if distance < nearest.0 {
                nearest = (distance, *value);
            }
        }
    }
    nearest.1
}

/// `cone`'s shell of potency band `band` as a mesh about its origin: along every direction the
/// point [`Potency::shell_radius`] gives for the disc's sample there, over [`SHELL_RINGS`] rings
/// and [`SHELL_SECTORS`] sectors of the disc out to its rim at the half-angle, closed back to the
/// origin by a fan along the rim. Flat-shaded, wound outward.
pub fn shell_mesh(cone: &Cone, band: Band, potency: &Potency) -> Mesh {
    let disc = &cone.disc;
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut directions: Vec<Vec3> = Vec::new();
    let mut vertex = |q: Vec2| {
        let along = disc.direction(q);
        let [reach, strength] = disc.sample(q).to_array();
        let emission = potency.emission(strength, cone.half_angle);
        positions.push((along * potency.shell_radius(band, reach, emission)).to_array());
        directions.push(along);
        (positions.len() - 1) as u32
    };
    let ring = |k: usize, j: usize| {
        let phi = std::f32::consts::TAU * (j % SHELL_SECTORS) as f32 / SHELL_SECTORS as f32;
        Vec2::from_angle(phi) * (k as f32 / SHELL_RINGS as f32)
    };
    let mut indices: Vec<u32> = Vec::new();
    let centre = vertex(Vec2::ZERO);
    let mut rows: Vec<Vec<u32>> = vec![vec![centre; SHELL_SECTORS]];
    for k in 1..=SHELL_RINGS {
        rows.push((0..SHELL_SECTORS).map(|j| vertex(ring(k, j))).collect());
    }
    for k in 0..SHELL_RINGS {
        for j in 0..SHELL_SECTORS {
            let next = (j + 1) % SHELL_SECTORS;
            let (a, b) = (rows[k][j], rows[k][next]);
            let (c, d) = (rows[k + 1][j], rows[k + 1][next]);
            if k > 0 {
                indices.extend([a, c, b]);
            }
            indices.extend([b, c, d]);
        }
    }
    let surface = indices.len();
    let origin = vertex(Vec2::ZERO);
    positions[origin as usize] = [0.0; 3];
    let rim = &rows[SHELL_RINGS];
    for j in 0..SHELL_SECTORS {
        indices.extend([origin, rim[j], rim[(j + 1) % SHELL_SECTORS]]);
    }
    wind_outward(&positions, &directions, &mut indices, surface, cone.axis);
    flat_mesh(positions, indices)
}

/// Turn every triangle of `indices` outward: the first `surface` indices are surface triangles,
/// wound so their corners' `directions` turn positively about the origin; the rest are rim
/// triangles, wound so their normal points away from `axis`.
fn wind_outward(
    positions: &[[f32; 3]],
    directions: &[Vec3],
    indices: &mut [u32],
    surface: usize,
    axis: Vec3,
) {
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
}

/// A flat-shaded triangle list of `positions` and `indices`.
fn flat_mesh(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_indices(Indices::U32(indices));
    mesh.duplicate_vertices();
    mesh.compute_flat_normals();
    mesh
}

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

    /// A fixture cone of `n` spiral directions about −Z, half-angle 25° and on-axis strength
    /// 10 ref-mm, direction `k` at disc point `q` reaching `reach(k, q)` with strength fraction
    /// `weight(q)`. Fixture data.
    fn fixture_cone(
        n: usize,
        reach: impl Fn(usize, Vec2) -> f32,
        weight: impl Fn(Vec2) -> f32,
    ) -> Cone {
        let axis = Vec3::NEG_Z;
        let half_angle = 25.0_f32.to_radians();
        let frame = Disc::of(axis, half_angle, 0.0, &[]);
        let rays = spiral(n)
            .into_iter()
            .enumerate()
            .map(|(k, q)| Ray {
                direction: frame.direction(q),
                reach: reach(k, q),
                weight: weight(q),
            })
            .collect();
        Cone::new(Vec3::ZERO, axis, half_angle, 10.0, 0.0, rays)
    }

    /// Fixture reaches: rising with the direction's index, and a third of the directions stopped
    /// at the origin among others stopped short. Under [`POTENCY`] both cross the band radii
    /// (1.41 m bright, 1.99 m mid at the flat fraction, DERIVED: `√(ρ · 5 / (Ω_c T))`).
    fn fixture_cones() -> [Cone; 2] {
        [
            fixture_cone(24, |k, _| 1.0 + 0.1 * k as f32, |_| 0.5),
            fixture_cone(
                24,
                |k, _| {
                    if k.is_multiple_of(3) {
                        0.0
                    } else {
                        0.5 + 0.2 * (k % 5) as f32
                    }
                },
                |q| 0.5 + 0.3 * q.x,
            ),
        ]
    }

    /// Fixture potency: the shipped figures' shape, not their source.
    const POTENCY: Potency = Potency {
        rate: 70.0,
        bright: 300.0,
        mid: 150.0,
    };

    fn positions(mesh: &Mesh) -> &[[f32; 3]] {
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions");
        };
        positions
    }

    /// The disc reads the record: a reach and a strength linear in the disc point come back
    /// exactly, at every point well inside the directions' hull and at every direction there.
    /// The footprint and the shells both read the record through it; a grid read on swapped axes,
    /// a wrong barycentric weight or a misplaced bilinear corner fails here. Fixture: the shipped
    /// cone's 64 directions; the four nodes about any point within 0.7 of the axis lie within
    /// 0.83 of it (DERIVED), inside the hull.
    #[test]
    fn the_disc_reproduces_what_is_linear_in_it() {
        let reach = |q: Vec2| 2.0 + 0.6 * q.x - 0.4 * q.y;
        let weight = |q: Vec2| 0.5 + 0.2 * q.y;
        let cone = fixture_cone(64, |_, q| reach(q), weight);
        let want = |q: Vec2| Vec2::new(reach(q), 10.0 * weight(q));
        let mut checked = 0;
        for i in -10..=10 {
            for j in -10..=10 {
                let q = Vec2::new(i as f32, j as f32) * 0.07;
                if q.length() > 0.7 {
                    continue;
                }
                let got = cone.disc.sample(q);
                assert!(
                    got.distance(want(q)) < 1e-4,
                    "{q}: {got} against {}",
                    want(q)
                );
                checked += 1;
            }
        }
        for ray in &cone.rays {
            let q = cone.disc.point(ray.direction);
            if q.length() <= 0.7 {
                let got = cone.disc.sample(q);
                assert!(got.distance(want(q)) < 1e-4, "{q}: {got}");
                checked += 1;
            }
        }
        assert!(checked > 300, "{checked}");
    }

    /// Every band shell is drawn through the disc: each vertex off the origin lies, along its own
    /// direction, at the shell radius of the disc's sample there. The footprint reads the same
    /// samples, so a shell built from the record's directions any other way parts from the
    /// colours on the surfaces it meets.
    #[test]
    fn every_shell_is_drawn_through_the_disc() {
        for cone in fixture_cones() {
            for band in Band::DRAWN {
                let mesh = shell_mesh(&cone, band, &POTENCY);
                let mut off_origin = 0;
                for p in positions(&mesh).iter().map(|p| Vec3::from(*p)) {
                    let Some(along) = p.try_normalize() else {
                        continue;
                    };
                    let [reach, strength] = cone.disc.sample(cone.disc.point(along)).to_array();
                    let e = POTENCY.emission(strength, cone.half_angle);
                    let want = POTENCY.shell_radius(band, reach, e);
                    assert!(
                        (p.length() - want).abs() < 1e-3,
                        "{band:?}: {p} against {want}"
                    );
                    off_origin += 1;
                }
                assert!(off_origin > 0, "{band:?}");
            }
        }
    }

    /// Every surface is closed and wound outward, however short its reaches: every edge is crossed
    /// once each way, and the volume it encloses, summed from its windings, is positive. A
    /// fragment's facing tells entering from leaving by it. Fixture reaches include directions
    /// stopped at the origin.
    #[test]
    fn every_surface_is_closed_and_wound_outward() {
        for cone in fixture_cones() {
            let shells = Band::DRAWN.map(|band| shell_mesh(&cone, band, &POTENCY));
            for mesh in std::iter::once(reach_mesh(&cone)).chain(shells) {
                let key = |p: [f32; 3]| p.map(|x| (x * 1.0e4).round() as i64);
                let mut edges = std::collections::HashMap::<([i64; 3], [i64; 3]), i32>::new();
                let mut volume = 0.0;
                for triangle in positions(&mesh).as_chunks::<3>().0 {
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
    }

    /// The reach surface passes through every recorded reach end.
    #[test]
    fn the_surface_holds_every_reach_end() {
        let [cone, _] = fixture_cones();
        let mesh = reach_mesh(&cone);
        for end in cone.rays.iter().map(|r| r.direction * r.reach) {
            assert!(
                positions(&mesh)
                    .iter()
                    .any(|p| Vec3::from(*p).distance(end) < 1e-4),
                "{end}"
            );
        }
    }
}
