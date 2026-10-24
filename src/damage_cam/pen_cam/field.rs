//! A field's recorded directions as one function of direction, and the surface drawn from it. A
//! field is a spall cone or a burst's; its support is a cap about its axis out to its half-angle
//! α, a ring's band from its inner angle out to α, or, for a ball whose rim is π, the whole sphere.
//!
//! The record samples the field at its directions. Each direction's place on the field's polar
//! disc — `q = (cos φ, sin φ) · θ/α`, polar angle θ over the half-angle α, azimuth φ about the
//! axis — carries the reach and strength the record gives it, interpolated linearly between the
//! record's directions: a drawing between the record's samples, never a direction the record does
//! not hold at a corner. A cap's and a band's directions are triangulated on the disc (Delaunay),
//! a band's triangles stopping at its inner edge, none spanning the hole, so nothing is
//! interpolated across it. The sphere's are triangulated on the sphere, by their convex hull, so
//! its interpolation has no rim and no seam: the disc's rim `|q| = 1` is the one antipode, and
//! every node on or past it holds the antipode's value. [`Disc`] bakes that function onto a grid
//! and reads it bilinearly; the look's volume and footprints read the same grid the same way, and
//! nothing off the support (`pen_cam_look.wgsl`, `disc_at`, [`Disc::at`]), and the reach surface
//! ([`reach_mesh`]) is drawn through its samples, so the surface, the volume inside it and the
//! footprints draw one function of direction.
//!
//! The reach surface joins, along every direction of the support, the point the disc's reach
//! gives there, closed back to the origin along the support's rims — a cap's one, a band's two,
//! the sphere's none: the boundary of the field's volume. A shadowed direction ends short, so a
//! shadow shows as a dent in it. Positions are relative to the field's origin, so the look's
//! material holds a front by distance from it.
//!
//! Every triangle is wound outward (counter-clockwise seen from outside the closed surface): a
//! surface triangle so its corners' directions turn positively about the origin, which holds
//! however short their reaches, a rim triangle so its normal points off the support, against the
//! axis on the outer rim and along it on the inner. So a fragment's facing tells an eye ray
//! entering the surface from one leaving it.

use std::f32::consts::{PI, TAU};

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use super::plan::{Cone, Ray};

/// The disc's grid: this many nodes a side over `q ∈ [−1, 1]²`, a node every 0.087 of the disc's
/// radius (DERIVED), against ~0.22 between the shipped cone's 64 directions (DERIVED: `√(π / 64)`).
/// A burst's 256 directions (`spall_cone.ron`) lie closer than the nodes: a ring of `λ = 0.45`,
/// its band 56° to 76°, ~0.09 rad apart against a node every 0.115 rad, three nodes across the
/// band; the sphere's ~0.22 rad apart against a node every 0.27 rad (all DERIVED). Even, so the
/// look packs two nodes a lane; `pen_cam_look.wgsl` declares the same side.
pub const DISC_SIDE: usize = 24;
/// The reach surface's rings out from the axis (from the inner edge, on a band) and sectors about
/// it: one a disc node's spacing apart on a cap (DERIVED from [`DISC_SIDE`]), closer on a band,
/// so the surface follows every node the volume reads.
const SURFACE_RINGS: usize = 12;
const SURFACE_SECTORS: usize = 64;

/// A field's recorded directions as one function over its polar disc: the disc's frame (the
/// field's unit axis, its half-angle and inner angle, and `u`, `v` across the axis, azimuth from
/// `u` toward `v`) and [`DISC_SIDE`]² nodes over `q ∈ [−1, 1]²`, row by row from `q.y = −1`, each
/// the `(reach m, strength ref-mm)` the triangulation of the record's directions interpolates
/// there. On a cap or a band a node outside the triangles takes the value at the nearest point of
/// their boundary; on the sphere a node takes the value along its direction, every node on or past
/// the rim the antipode's.
#[derive(Clone, Debug)]
pub struct Disc {
    pub axis: Vec3,
    pub half_angle: f32,
    /// The support's inner edge (rad off the axis): a band's near edge, 0 on a cap and the sphere.
    pub inner: f32,
    pub u: Vec3,
    pub v: Vec3,
    pub nodes: Vec<Vec2>,
}

impl Disc {
    /// The disc of a field about unit `axis` whose support lies from `inner` to `half_angle` (rad)
    /// off it, the whole sphere at a half-angle of π, whose strength is `strength` (ref-mm),
    /// sampled at `rays`.
    pub fn of(axis: Vec3, half_angle: f32, inner: f32, strength: f32, rays: &[Ray]) -> Self {
        let (u, v) = axis.any_orthonormal_pair();
        let mut disc = Self {
            axis,
            half_angle,
            inner,
            u,
            v,
            nodes: Vec::new(),
        };
        let values: Vec<Vec2> = rays
            .iter()
            .map(|ray| Vec2::new(ray.reach, ray.weight * strength))
            .collect();
        let step = 2.0 / (DISC_SIDE - 1) as f32;
        let grid = (0..DISC_SIDE * DISC_SIDE)
            .map(|k| Vec2::new((k % DISC_SIDE) as f32, (k / DISC_SIDE) as f32) * step - 1.0);
        disc.nodes = if disc.whole_sphere() {
            let directions: Vec<Vec3> = rays.iter().map(|ray| ray.direction).collect();
            let faces = sphere_faces(&directions, axis);
            grid.map(|q| {
                let along = disc.direction(q.clamp_length_max(1.0));
                on_sphere(&directions, &values, &faces, along)
            })
            .collect()
        } else {
            let points: Vec<Vec2> = rays.iter().map(|ray| disc.point(ray.direction)).collect();
            let hole = disc.hole();
            let mut triangles = delaunay(&points);
            triangles
                .retain(|t| (points[t[0]] + points[t[1]] + points[t[2]]).length() >= 3.0 * hole);
            let boundary = hull_edges(&triangles);
            grid.map(|q| interpolate(&points, &values, &triangles, &boundary, q))
                .collect()
        };
        disc
    }

    /// The share of the disc's radius inside the support's inner edge: 0 on a cap and the sphere.
    /// A band's triangle whose centroid lies inside it spans the hole.
    fn hole(&self) -> f32 {
        self.inner / self.half_angle.max(1e-3)
    }

    /// The support is the whole sphere: a half-angle of π, which a ball whose ratio is below 1
    /// takes exactly (`FieldDescriptor::half_angle`).
    fn whole_sphere(&self) -> bool {
        self.half_angle >= PI
    }

    /// The disc point of unit `direction`: `(cos φ, sin φ) · θ/α`; the antipode, which has no
    /// azimuth, at `(π/α, 0)`, on the sphere a point of the rim.
    pub fn point(&self, direction: Vec3) -> Vec2 {
        let theta = direction.dot(self.axis).clamp(-1.0, 1.0).acos();
        let across = Vec2::new(direction.dot(self.u), direction.dot(self.v));
        across.normalize_or(Vec2::X) * (theta / self.half_angle.max(1e-3))
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

    /// The `(reach m, strength ref-mm)` along unit `direction`: [`Self::sample`] at its disc point
    /// on the support, zero off it, short of the inner edge or past the rim. The look's shader
    /// reads the disc so at runtime (`pen_cam_look.wgsl`, `polar` and `disc_at`); this is the read
    /// it mirrors, held by the tests.
    pub fn at(&self, direction: Vec3) -> Vec2 {
        let theta = direction.dot(self.axis).clamp(-1.0, 1.0).acos();
        if theta < self.inner || theta > self.half_angle {
            return Vec2::ZERO;
        }
        self.sample(self.point(direction))
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

/// The faces of the convex hull of unit `directions`, as index triples wound outward
/// (`a · (b × c) > 0`): the Delaunay triangles of their stereographic projection from the
/// direction nearest `−axis`, which maps the circle through each hull face's corners to the
/// triangle's circumcircle, and that direction fanned to the projection's boundary. Fewer than
/// four directions give none.
fn sphere_faces(directions: &[Vec3], axis: Vec3) -> Vec<[usize; 3]> {
    if directions.len() < 4 {
        return Vec::new();
    }
    let pole = (0..directions.len())
        .min_by(|&a, &b| directions[a].dot(axis).total_cmp(&directions[b].dot(axis)))
        .unwrap_or(0);
    let p = directions[pole];
    let (e1, e2) = p.any_orthonormal_pair();
    let others: Vec<usize> = (0..directions.len()).filter(|&k| k != pole).collect();
    let points: Vec<Vec2> = others
        .iter()
        .map(|&k| {
            let d = directions[k];
            Vec2::new(d.dot(e1), d.dot(e2)) / (1.0 - d.dot(p)).max(1e-6)
        })
        .collect();
    let planar = delaunay(&points);
    let fan: Vec<[usize; 3]> = hull_edges(&planar)
        .into_iter()
        .map(|(a, b)| [pole, others[a], others[b]])
        .collect();
    planar
        .iter()
        .map(|t| t.map(|i| others[i]))
        .chain(fan)
        .map(|[a, b, c]| {
            if directions[a].dot(directions[b].cross(directions[c])) < 0.0 {
                [a, c, b]
            } else {
                [a, b, c]
            }
        })
        .collect()
}

/// The value along unit `direction` that hull `faces` of unit `directions` (carrying `values`)
/// interpolate: linear across the face whose cone about the origin holds it, its corners weighted
/// barycentrically at the point where the direction meets the face's plane; outside every face's
/// cone (a hull that does not wrap the origin), the face it lies least outside, its weights held
/// non-negative; with no faces the nearest direction's value; zero with no directions.
fn on_sphere(directions: &[Vec3], values: &[Vec2], faces: &[[usize; 3]], direction: Vec3) -> Vec2 {
    const INSIDE: f32 = -1e-5;
    let blend =
        |[a, b, c]: [usize; 3], w: Vec3| values[a] * w.x + values[b] * w.y + values[c] * w.z;
    let mut best = (f32::NEG_INFINITY, None);
    for &[a, b, c] in faces {
        let (da, db, dc) = (directions[a], directions[b], directions[c]);
        let w = Vec3::new(
            direction.dot(db.cross(dc)),
            da.dot(direction.cross(dc)),
            da.dot(db.cross(direction)),
        );
        let sum = w.x + w.y + w.z;
        if sum <= 0.0 {
            continue;
        }
        let w = w / sum;
        let low = w.min_element();
        if low >= INSIDE {
            return blend([a, b, c], w);
        }
        if low > best.0 {
            let held = w.max(Vec3::ZERO);
            best = (
                low,
                Some(blend([a, b, c], held / (held.x + held.y + held.z))),
            );
        }
    }
    if let Some(value) = best.1 {
        return value;
    }
    directions
        .iter()
        .zip(values)
        .max_by(|(a, _), (b, _)| a.dot(direction).total_cmp(&b.dot(direction)))
        .map_or(Vec2::ZERO, |(_, value)| *value)
}

/// `cone`'s reach surface as a mesh about its origin: along every direction of the support the
/// disc's reach there, over [`SURFACE_RINGS`] rings and [`SURFACE_SECTORS`] sectors of the disc
/// from the axis (a band's inner edge) out to its rim at the half-angle, closed back to the origin
/// by a fan along each rim: a cap's one, a band's two, the sphere's none, whose last ring is the
/// antipode. Flat-shaded, wound outward.
pub fn reach_mesh(cone: &Cone) -> Mesh {
    let disc = &cone.disc;
    let (hole, sphere) = (disc.hole(), disc.whole_sphere());
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut directions: Vec<Vec3> = Vec::new();
    let mut vertex = |q: Vec2| {
        let along = disc.direction(q);
        positions.push((along * disc.sample(q).x).to_array());
        directions.push(along);
        (positions.len() - 1) as u32
    };
    let ring = |k: usize, j: usize| {
        let phi = TAU * (j % SURFACE_SECTORS) as f32 / SURFACE_SECTORS as f32;
        Vec2::from_angle(phi) * (hole + (1.0 - hole) * (k as f32 / SURFACE_RINGS as f32))
    };
    let rows: Vec<Vec<u32>> = (0..=SURFACE_RINGS)
        .map(|k| {
            if (k == 0 && hole == 0.0) || (k == SURFACE_RINGS && sphere) {
                vec![vertex(ring(k, 0)); SURFACE_SECTORS]
            } else {
                (0..SURFACE_SECTORS).map(|j| vertex(ring(k, j))).collect()
            }
        })
        .collect();
    let mut indices: Vec<u32> = Vec::new();
    for k in 0..SURFACE_RINGS {
        for j in 0..SURFACE_SECTORS {
            let next = (j + 1) % SURFACE_SECTORS;
            let (a, b) = (rows[k][j], rows[k][next]);
            let (c, d) = (rows[k + 1][j], rows[k + 1][next]);
            for [x, y, z] in [[a, c, b], [b, c, d]] {
                if x != y && y != z && z != x {
                    indices.extend([x, y, z]);
                }
            }
        }
    }
    let surface = indices.len();
    let mut outer = surface;
    let rims = [
        (!sphere).then_some(SURFACE_RINGS),
        (hole > 0.0).then_some(0),
    ];
    if rims.iter().any(Option::is_some) {
        let origin = vertex(Vec2::ZERO);
        positions[origin as usize] = [0.0; 3];
        for (n, row) in rims.into_iter().enumerate() {
            let Some(row) = row else {
                continue;
            };
            let rim = &rows[row];
            for j in 0..SURFACE_SECTORS {
                indices.extend([origin, rim[j], rim[(j + 1) % SURFACE_SECTORS]]);
            }
            if n == 0 {
                outer = indices.len();
            }
        }
    }
    wind_outward(
        &positions,
        &directions,
        &mut indices,
        surface,
        outer,
        cone.axis,
    );
    flat_mesh(positions, indices)
}

/// Turn every triangle of `indices` outward: the first `surface` indices are surface triangles,
/// wound so their corners' `directions` turn positively about the origin; those up to `outer`
/// the outer rim's, wound so their normal points against `axis`, off a support that lies short of
/// the rim; the rest the inner rim's, wound so their normal points along `axis`, off a support
/// that lies past the inner edge.
fn wind_outward(
    positions: &[[f32; 3]],
    directions: &[Vec3],
    indices: &mut [u32],
    surface: usize,
    outer: usize,
    axis: Vec3,
) {
    for (at, triangle) in indices.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        let [a, b, c] = [0, 1, 2].map(|k| triangle[k] as usize);
        let inward = if 3 * at < surface {
            directions[a].dot(directions[b].cross(directions[c])) < 0.0
        } else {
            let point = |i: usize| Vec3::from(positions[i]);
            let along = (point(b) - point(a)).cross(point(c) - point(a)).dot(axis);
            if 3 * at < outer {
                along > 0.0
            } else {
                along < 0.0
            }
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
pub(super) mod tests {
    use super::super::plan::Thrown;
    use super::*;

    /// A fixture field's throw: at the line's start, at record time zero, its front at 1 m/s. These
    /// tests read a field's shape, never its timing. Fixture data.
    pub(in super::super) const AT_ORIGIN: Thrown = Thrown {
        arc: 0.0,
        t: 0.0,
        speed: 1.0,
    };

    /// Golden-spiral points, as the cone's strata place its directions.
    fn spiral(n: usize) -> Vec<Vec2> {
        let golden = PI * (3.0 - 5.0_f32.sqrt());
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

    /// A fixture cone of `n` spiral directions about −Z, half-angle 25° and strength
    /// 10 ref-mm, direction `k` at disc point `q` reaching `reach(k, q)` with strength fraction
    /// `weight(q)`. Fixture data.
    fn fixture_cone(
        n: usize,
        reach: impl Fn(usize, Vec2) -> f32,
        weight: impl Fn(Vec2) -> f32,
    ) -> Cone {
        let axis = Vec3::NEG_Z;
        let half_angle = 25.0_f32.to_radians();
        let frame = Disc::of(axis, half_angle, 0.0, 0.0, &[]);
        let rays = spiral(n)
            .into_iter()
            .enumerate()
            .map(|(k, q)| Ray {
                direction: frame.direction(q),
                reach: reach(k, q),
                weight: weight(q),
            })
            .collect();
        Cone::new(Vec3::ZERO, axis, half_angle, 0.0, 10.0, AT_ORIGIN, rays)
    }

    /// A fixture field of `n` directions about −Z over the support from `inner` to `half_angle`
    /// (rad), strength 10 ref-mm, in equal-solid-angle strata (`cos θ` even across the support,
    /// the azimuth turning by the golden angle), direction `k` reaching `reach(k, direction)`,
    /// thrown with strength fraction ½. Fixture data.
    fn fixture_field(
        half_angle: f32,
        inner: f32,
        n: usize,
        reach: impl Fn(usize, Vec3) -> f32,
    ) -> Cone {
        let axis = Vec3::NEG_Z;
        let frame = Disc::of(axis, half_angle, inner, 0.0, &[]);
        let golden = PI * (3.0 - 5.0_f32.sqrt());
        let (near, far) = (inner.cos(), half_angle.cos());
        let rays = (0..n)
            .map(|k| {
                let c = near + (far - near) * (k as f32 + 0.5) / n as f32;
                let phi = k as f32 * golden;
                let across = frame.u * phi.cos() + frame.v * phi.sin();
                let direction = (axis * c + across * (1.0 - c * c).max(0.0).sqrt()).normalize();
                Ray {
                    direction,
                    reach: reach(k, direction),
                    weight: 0.5,
                }
            })
            .collect();
        Cone::new(Vec3::ZERO, axis, half_angle, inner, 10.0, AT_ORIGIN, rays)
    }

    /// A ring's band, 56° to 76° off the axis (a ring of `λ = 0.45` and half-width 10°), over
    /// 128 directions. Fixture data.
    fn band(reach: impl Fn(usize, Vec3) -> f32) -> Cone {
        fixture_field(76.0_f32.to_radians(), 56.0_f32.to_radians(), 128, reach)
    }

    /// The whole sphere over 128 directions. Fixture data.
    fn sphere(reach: impl Fn(usize, Vec3) -> f32) -> Cone {
        fixture_field(PI, 0.0, 128, reach)
    }

    /// Fixture reaches: rising with the direction's index, and a third of the directions stopped
    /// at the origin among others stopped short, beside directions that run on; on caps, a band
    /// and the sphere.
    fn fixture_cones() -> [Cone; 4] {
        let stopped = |k: usize| {
            if k.is_multiple_of(3) {
                0.0
            } else {
                0.5 + 0.2 * (k % 5) as f32
            }
        };
        [
            fixture_cone(24, |k, _| 1.0 + 0.1 * k as f32, |_| 0.5),
            fixture_cone(24, |k, _| stopped(k), |q| 0.5 + 0.3 * q.x),
            band(|k, _| stopped(k)),
            sphere(|k, _| stopped(k)),
        ]
    }

    /// The disc point of grid node `k`.
    fn node_point(k: usize) -> Vec2 {
        let step = 2.0 / (DISC_SIDE - 1) as f32;
        Vec2::new((k % DISC_SIDE) as f32, (k / DISC_SIDE) as f32) * step - 1.0
    }

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
    /// The volume, its surface and the footprint all read the record through it; a grid read on
    /// swapped axes, a wrong barycentric weight or a misplaced bilinear corner fails here.
    /// Fixture: the shipped cone's 64 directions; the four nodes about any point within 0.7 of the
    /// axis lie within 0.83 of it (DERIVED), inside the hull.
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

    /// A band holds nothing off it: the disc reads zero along every direction short of its inner
    /// edge or past its rim and reads the band on it, and the reach surface has no vertex off the
    /// band but the origin. The look's volume and footprints read the disc so
    /// (`pen_cam_look.wgsl`, `polar`); a band drawn as the cap it lies in fills the cone about the
    /// line, where the ring threw nothing. Directions sampled half a degree off each edge, against
    /// an f32 angle good to 1e-5 rad.
    #[test]
    fn a_band_holds_nothing_off_it() {
        let cone = band(|_, _| 1.0);
        let disc = &cone.disc;
        for k in 0..180 {
            let theta = (k as f32 + 0.5).to_radians();
            for j in 0..8 {
                let phi = j as f32 * TAU / 8.0;
                let across = disc.u * phi.cos() + disc.v * phi.sin();
                let direction = disc.axis * theta.cos() + across * theta.sin();
                let value = disc.at(direction);
                if theta < disc.inner || theta > disc.half_angle {
                    assert_eq!(value, Vec2::ZERO, "{k}.5°");
                } else {
                    assert!((value.x - 1.0).abs() < 1e-4, "{k}.5°: {value}");
                }
            }
        }
        let mut on_band = 0;
        for p in positions(&reach_mesh(&cone)).iter().map(|p| Vec3::from(*p)) {
            let Some(along) = p.try_normalize() else {
                continue;
            };
            let theta = along.angle_between(disc.axis);
            assert!(
                theta > disc.inner - 1e-4 && theta < disc.half_angle + 1e-4,
                "a vertex {}° off the axis",
                theta.to_degrees()
            );
            on_band += 1;
        }
        assert!(on_band > 0);
    }

    /// A band's triangulation stops at its inner edge: a node in the hole takes the value of the
    /// band's nearest edge, never a blend across the hole of the band's two sides. The look reads
    /// the disc bilinearly, so the nodes in the hole are what the band's inner edge draws; a
    /// triangle spanning the hole carries the far side's reach into it. Fixture: reach 3 on the
    /// band's `+u` side, 1 on its `−u` side; nodes well inside the hole (within 0.9 of its edge)
    /// and within 60° of either side's middle.
    #[test]
    fn a_band_is_not_interpolated_across_its_hole() {
        let cone = band(|_, d| if d.x > 0.0 { 3.0 } else { 1.0 });
        let disc = &cone.disc;
        assert_eq!(disc.u, Vec3::X, "the fixture's sides are its frame's");
        let hole = disc.inner / disc.half_angle;
        let mut checked = 0;
        for (k, node) in disc.nodes.iter().enumerate() {
            let q = node_point(k);
            if q.length() >= 0.9 * hole {
                continue;
            }
            let phi = q.y.atan2(q.x).abs().to_degrees();
            let want = if phi < 60.0 {
                3.0
            } else if phi > 120.0 {
                1.0
            } else {
                continue;
            };
            assert!(
                (node.x - want).abs() < 1e-4,
                "{q}: {} against {want}",
                node.x
            );
            checked += 1;
        }
        assert!(checked > 100, "{checked}");
    }

    /// The sphere's disc has no seam at its rim and no smear at its antipode: every node on or
    /// past the rim, where all the disc's azimuths meet at the antipode, holds one value, and the
    /// antipode reads the value of the directions about it. The look reads the disc bilinearly
    /// across the rim, so a rim whose nodes differ draws a seam of different reaches and strengths
    /// about the antipode; an antipode read at the disc's centre draws the axis's value there.
    /// Fixture: a reach turning with azimuth, `2 + 0.5 d·u + 0.3 d·axis`, so a rim taken from the
    /// triangulation's boundary differs at every azimuth, and the axis's 2.3 is not the
    /// antipode's 1.7. Tolerance: the face's plane scales a linear term by `cos r` at worst, `r`
    /// its corners' angular radius, so 0.3 (1 − cos 0.6) = 0.05 bounds faces twice the strata's
    /// spacing `√(4π / 128)` = 0.31 rad (DERIVED).
    #[test]
    fn the_sphere_has_no_seam_at_its_antipode() {
        let axis = Vec3::NEG_Z;
        let reach = |d: Vec3| 2.0 + 0.5 * d.x + 0.3 * d.dot(axis);
        let cone = sphere(|_, d| reach(d));
        let disc = &cone.disc;
        assert_eq!(
            disc.u,
            Vec3::X,
            "the fixture's azimuth turns from its frame's u"
        );
        let rim: Vec<f32> = (0..DISC_SIDE * DISC_SIDE)
            .filter(|&k| node_point(k).length() >= 1.0)
            .map(|k| disc.nodes[k].x)
            .collect();
        assert!(rim.len() > 4 * (DISC_SIDE - 1), "{}", rim.len());
        let (lo, hi) = rim
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
        assert!(hi - lo < 1e-5, "the rim's nodes span {lo} to {hi}");
        let antipode = disc.sample(disc.point(-axis)).x;
        assert!(
            (antipode - reach(-axis)).abs() < 0.05,
            "the antipode reads {antipode} against {}",
            reach(-axis)
        );
    }

    /// The reach surface is drawn through the disc: each vertex off the origin lies, along its own
    /// direction, at the disc's reach there. The volume and the footprint read the same samples,
    /// so a surface built from the record's directions any other way parts from the volume the
    /// look marches inside it and from the colours on the surfaces it meets.
    #[test]
    fn the_reach_surface_is_drawn_through_the_disc() {
        for cone in fixture_cones() {
            let mesh = reach_mesh(&cone);
            let mut off_origin = 0;
            for p in positions(&mesh).iter().map(|p| Vec3::from(*p)) {
                let Some(along) = p.try_normalize() else {
                    continue;
                };
                let want = cone.disc.sample(cone.disc.point(along)).x;
                assert!((p.length() - want).abs() < 1e-3, "{p} against {want}");
                off_origin += 1;
            }
            assert!(off_origin > 0);
        }
    }

    /// The reach surface is closed and wound outward, however short its reaches and whatever its
    /// support: every edge is crossed once each way, and the volume it encloses, summed from its
    /// windings, is positive. The look draws the field from the faces where an eye ray leaves the
    /// volume, told by their facing; an open or inward-wound surface drops or doubles a stretch of
    /// the field. Fixture reaches include directions stopped at the origin; supports a cap, a band
    /// (two rims, the inner one facing along the axis) and the sphere (none).
    #[test]
    fn the_reach_surface_is_closed_and_wound_outward() {
        for cone in fixture_cones() {
            let mesh = reach_mesh(&cone);
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
