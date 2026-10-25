//! A field's recorded directions as one function of direction, baked onto a direction map. A field
//! is a spall cone or a burst's; its support is a cap about its axis out to its half-angle α, a
//! ring's band from its inner angle out to α, or, for a ball whose rim is π, the whole sphere.
//!
//! The record samples the field at its directions, each with the reach and strength the record
//! gives it. Between them the strength is the record's linear interpolation, and so is the reach
//! wherever the directions about a point ended alike: inside one hard body, past one, or free of
//! every body. Where they ended unlike, a shadow's edge lies between them, and the struck tank's
//! hard bodies ([`HardBodies`], every non-witness ballistic primitive, posed as the record has the
//! tank) place it: the reach follows the directions that ended as the point's own line would.
//! [`Samples::at`] states that contract; the design narrative is design doc §5, "Potency colour"
//! (`.agents/docs/design/armor-penetration-and-damage.md`).
//!
//! A cap's and a band's directions are triangulated on the polar disc `q = (cos φ, sin φ) · θ/α`
//! (Delaunay), a band's triangles stopping at its inner edge, none spanning the hole; the sphere's
//! on the sphere, by their convex hull, so its interpolation has no rim and no seam.
//!
//! [`Map`] bakes the function onto an octahedral map about the field's axis, [`MAP_TEXELS`]² texels
//! with a one-texel gutter, read bilinearly; [`atlas`] packs a record's maps into the one texture
//! the look's shader reads them from (`pen_cam_look.wgsl`, `map_at`), and [`Map::sample`] is the
//! same read on the CPU. [`bake`] bakes every field of a record, its rows spread over the
//! machine's threads.

use std::f32::consts::{FRAC_PI_2, PI, SQRT_2};
use std::sync::Mutex;

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::plan::{Cone, Ray};
use crate::damage_cam::shapes::HardBodies;

/// The direction map's texels a side inside its gutter. A cap's map covers its support's bounding
/// square of octahedral coordinates: a 26.2° spall cone's ~0.24° a texel at the axis (DERIVED:
/// `2 · zoom / 256` rad, `zoom = sin α / (sin α + cos α)` = 0.33), against ~5.5° between its 64
/// directions (MEASURED over the shipped records); the whole sphere's ~0.8° a texel (DERIVED,
/// `√(4π / 256²)`).
pub const MAP_SIDE: usize = 256;
/// The map's texels a side with its gutter, one texel each side holding the value just past the
/// map's edge (across the octahedron's fold on the sphere), so a bilinear read at the edge blends
/// what lies across it.
pub const MAP_TEXELS: usize = MAP_SIDE + 2;
/// The most fields one record's atlas holds: two tiles a side.
pub const MAP_FIELDS: usize = 4;
const ATLAS_TILES: usize = 2;
/// How far (m) the guide looks along a direction for a body: past every tank's extent (DERIVED:
/// no tank spans a kilometre), so a body a direction can meet is never past it.
const GUIDE_REACH_M: f32 = 1.0e3;
/// A record's direction counts as ended in the body the guide meets first when its reach lies from
/// this far (m) short of the body's entry to this far past its exit: the sim walks the same
/// triangles and ends a stopped direction inside the body.
const ENDING_SLACK_M: f32 = 1.0e-3;
/// The most a direction turns (rad) per unit of octahedral coordinate (DERIVED: along a face's
/// diagonal at its centre, where `p = (1/3, 1/3, 1/3)` is `1/√3` long and its step
/// `(1, 1, −2)/√2` is `√3` long and square to it; MEASURED 2.999 over 4·10⁵ random steps).
const OCTAHEDRAL_TURN: f32 = 3.0;
/// The texel diagonals past the support's edges a map bakes, each at most
/// `OCTAHEDRAL_TURN · √2 · 2 · zoom / MAP_SIDE` rad: a bilinear read on the support blends texels at
/// most one diagonal off it, and the look reads the map on the support alone (`pen_cam_look.wgsl`,
/// `field_at`). Every texel farther off holds zero.
const SUPPORT_MARGIN_DIAGONALS: f32 = 1.0;

/// A field's recorded directions as one function of direction, baked onto an octahedral map about
/// the field's axis: [`MAP_TEXELS`]² texels, row by row, each the `(reach m, strength ref-mm)` the
/// record gives along the texel's direction ([`Samples::at`]). The map covers the square of
/// octahedral coordinates `[−zoom, zoom]²`, the support's bounding square for a support short of
/// the hemisphere and the whole octahedron (`zoom = 1`) otherwise, so the sphere's map has no pole
/// and no rim: its only edge is the octahedron's fold, which the gutter wraps.
#[derive(Clone, Debug)]
pub struct Map {
    pub axis: Vec3,
    pub u: Vec3,
    pub v: Vec3,
    pub half_angle: f32,
    /// The support's inner edge (rad off the axis): a band's near edge, 0 on a cap and the sphere.
    pub inner: f32,
    pub zoom: f32,
    pub texels: Vec<Vec2>,
    /// The longest reach any texel holds (m): no direction of the field reaches past it.
    pub longest: f32,
}

impl Map {
    /// `cone`'s map, its shadows' edges placed between its directions by `bodies`, the struck
    /// tank's hard bodies in the record's frame, where the record's directions leave them open;
    /// without bodies, the record's interpolation alone.
    pub fn of(cone: &Cone, bodies: Option<&HardBodies>) -> Self {
        let (u, v) = cone.axis.any_orthonormal_pair();
        let zoom = if cone.half_angle >= FRAC_PI_2 {
            1.0
        } else {
            let (s, c) = cone.half_angle.sin_cos();
            s / (s + c)
        };
        let mut map = Self {
            axis: cone.axis,
            u,
            v,
            half_angle: cone.half_angle,
            inner: cone.inner,
            zoom,
            texels: Vec::new(),
            longest: 0.0,
        };
        let samples = Samples::of(&map, cone.origin, cone.strength, &cone.rays, bodies);
        let mut texels = vec![Vec2::ZERO; MAP_TEXELS * MAP_TEXELS];
        // Rows go to the threads two at a time, as each frees, so a run of texels the guide
        // decides (a silhouette's rows) is shared out.
        let rows = 2;
        let work = Mutex::new(texels.chunks_mut(rows * MAP_TEXELS).enumerate());
        let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    let mut cache = 0;
                    loop {
                        let next = work.lock().map(|mut work| work.next());
                        let Ok(Some((chunk, block))) = next else {
                            break;
                        };
                        for (k, texel) in block.iter_mut().enumerate() {
                            let index = chunk * rows * MAP_TEXELS + k;
                            let direction =
                                map.texel_direction(index % MAP_TEXELS, index / MAP_TEXELS);
                            if map.near_support(direction) {
                                *texel = samples.at(direction, &mut cache);
                            }
                        }
                    }
                });
            }
        });
        map.longest = texels.iter().map(|texel| texel.x).fold(0.0, f32::max);
        map.texels = texels;
        map
    }

    /// The support is the whole sphere: a half-angle of π, which a ball whose ratio is below 1
    /// takes exactly (`FieldDescriptor::half_angle`).
    pub fn whole_sphere(&self) -> bool {
        self.half_angle >= PI
    }

    /// Unit `direction` in the map's frame: across it along `u` and `v`, along the axis.
    fn local(&self, direction: Vec3) -> Vec3 {
        Vec3::new(
            direction.dot(self.u),
            direction.dot(self.v),
            direction.dot(self.axis),
        )
    }

    /// The map coordinate of unit `direction`: its octahedral coordinate over the zoom, in
    /// `[−1, 1]²` on the map.
    pub fn coordinate(&self, direction: Vec3) -> Vec2 {
        octahedral(self.local(direction)) / self.zoom
    }

    /// The unit direction at the centre of texel `(i, j)`, gutter included.
    fn texel_direction(&self, i: usize, j: usize) -> Vec3 {
        let o = ((Vec2::new(i as f32, j as f32) - 0.5) / MAP_SIDE as f32 * 2.0 - 1.0) * self.zoom;
        let local = unfold(wrap(o));
        self.u * local.x + self.v * local.y + self.axis * local.z
    }

    /// Unit `direction` lies within [`SUPPORT_MARGIN_DIAGONALS`] of the support.
    fn near_support(&self, direction: Vec3) -> bool {
        let diagonal = OCTAHEDRAL_TURN * SQRT_2 * 2.0 * self.zoom / MAP_SIDE as f32;
        let margin = SUPPORT_MARGIN_DIAGONALS * diagonal;
        let theta = direction.dot(self.axis).clamp(-1.0, 1.0).acos();
        theta >= self.inner - margin && theta <= self.half_angle + margin
    }

    /// The `(reach m, strength ref-mm)` along unit `direction`, bilinear between the four texels
    /// about its map coordinate, as a texture unit filters them: the look's shader reads the map so
    /// (`pen_cam_look.wgsl`, `map_at`); this is the read it mirrors, held by the tests.
    pub fn sample(&self, direction: Vec3) -> Vec2 {
        let top = (MAP_TEXELS - 1) as f32;
        let g = ((self.coordinate(direction) + 1.0) * 0.5 * MAP_SIDE as f32 + 0.5)
            .clamp(Vec2::ZERO, Vec2::splat(top));
        let low = g.floor().min(Vec2::splat(top - 1.0));
        let t = g - low;
        let (i, j) = (low.x as usize, low.y as usize);
        let at = |i: usize, j: usize| self.texels[j * MAP_TEXELS + i];
        let below = at(i, j).lerp(at(i + 1, j), t.x);
        let above = at(i, j + 1).lerp(at(i + 1, j + 1), t.x);
        below.lerp(above, t.y)
    }

    /// The `(reach m, strength ref-mm)` along unit `direction`: [`Self::sample`] on the support,
    /// zero off it, short of the inner edge or past the rim.
    pub fn at(&self, direction: Vec3) -> Vec2 {
        let theta = direction.dot(self.axis).clamp(-1.0, 1.0).acos();
        if theta < self.inner || theta > self.half_angle {
            return Vec2::ZERO;
        }
        self.sample(direction)
    }
}

/// Every field of `cones` baked onto its map ([`Map::of`]), guided by `bodies`.
pub fn bake(cones: &[Cone], bodies: Option<&HardBodies>) -> Vec<Map> {
    cones.iter().map(|cone| Map::of(cone, bodies)).collect()
}

/// The texel at which field `index`'s tile starts in the atlas.
pub fn tile(index: usize) -> UVec2 {
    UVec2::new((index % ATLAS_TILES) as u32, (index / ATLAS_TILES) as u32) * MAP_TEXELS as u32
}

/// The first [`MAP_FIELDS`] of `maps` as one texture: field `k`'s map in tile [`tile`]`(k)`, each
/// texel's reach (m) in red and strength (ref-mm) in green as half floats, read with linear
/// filtering and clamped at the edge; an unused tile holds zero.
pub fn atlas(maps: &[Map]) -> Image {
    let side = (ATLAS_TILES * MAP_TEXELS) as u32;
    let mut image = Image::new_fill(
        Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0; 4],
        TextureFormat::Rg16Float,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    for (index, map) in maps.iter().take(MAP_FIELDS).enumerate() {
        let origin = tile(index);
        for (k, texel) in map.texels.iter().enumerate() {
            let at = origin + UVec2::new((k % MAP_TEXELS) as u32, (k / MAP_TEXELS) as u32);
            let colour = Color::LinearRgba(LinearRgba::new(texel.x, texel.y, 0.0, 0.0));
            image
                .set_color_at(at.x, at.y, colour)
                .expect("an Rg16Float texel inside the atlas");
        }
    }
    image
}

/// The octahedral coordinate of unit `local` (z the axis): the upper hemisphere on the diamond
/// `|x| + |y| ≤ 1`, the lower folded out to the square's corners, the antipode at all four.
fn octahedral(local: Vec3) -> Vec2 {
    let p = local / (local.x.abs() + local.y.abs() + local.z.abs()).max(1e-12);
    if p.z >= 0.0 {
        Vec2::new(p.x, p.y)
    } else {
        (Vec2::ONE - Vec2::new(p.y, p.x).abs()) * sign_not_zero(Vec2::new(p.x, p.y))
    }
}

/// The unit direction (z the axis) at octahedral coordinate `o` in `[−1, 1]²`.
fn unfold(o: Vec2) -> Vec3 {
    let z = 1.0 - o.x.abs() - o.y.abs();
    let xy = if z >= 0.0 {
        o
    } else {
        (Vec2::ONE - Vec2::new(o.y, o.x).abs()) * sign_not_zero(o)
    };
    Vec3::new(xy.x, xy.y, z).normalize()
}

/// Octahedral coordinate `o` past the square's edge, brought back across the fold: the edge
/// `x = 1` folds `y` onto `−y`, and so on each side.
fn wrap(o: Vec2) -> Vec2 {
    let mut o = o;
    if o.x.abs() > 1.0 {
        o = Vec2::new(o.x.signum() * 2.0 - o.x, -o.y);
    }
    if o.y.abs() > 1.0 {
        o = Vec2::new(-o.x, o.y.signum() * 2.0 - o.y);
    }
    o
}

fn sign_not_zero(v: Vec2) -> Vec2 {
    Vec2::new(
        if v.x >= 0.0 { 1.0 } else { -1.0 },
        if v.y >= 0.0 { 1.0 } else { -1.0 },
    )
}

/// How a record's direction ended, as the guide sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ending {
    /// The guide meets no body along it.
    Free,
    /// It ended inside `body`, the first the guide meets along it, `depth` (m) past its entry.
    Within { body: u32, depth: f32 },
    /// The guide meets `body` first, and the record ends it elsewhere: short of it, or past it.
    Past { body: u32 },
}

impl Ending {
    /// The first body the guide meets along it.
    fn body(self) -> Option<u32> {
        match self {
            Ending::Free => None,
            Ending::Within { body, .. } | Ending::Past { body } => Some(body),
        }
    }

    /// The two ended alike: both free, both within one body, or both past one.
    fn alike(self, other: Self) -> bool {
        match (self, other) {
            (Ending::Free, Ending::Free) => true,
            (Ending::Within { body: a, .. }, Ending::Within { body: b, .. })
            | (Ending::Past { body: a }, Ending::Past { body: b }) => a == b,
            _ => false,
        }
    }
}

/// A field's record as one function of direction: its directions, each one's `(reach m, strength
/// ref-mm)`, their triangulation, and with the struck tank's hard bodies, how each ended.
struct Samples<'a> {
    origin: Vec3,
    directions: Vec<Vec3>,
    values: Vec<Vec2>,
    mesh: Triangulation,
    /// Each direction's triangles: where a point's search starts after the last point's triangle.
    incident: Vec<Vec<usize>>,
    endings: Vec<Ending>,
    bodies: Option<&'a HardBodies>,
}

/// The record's directions triangulated: a cap's and a band's on the polar disc
/// `q = (cos φ, sin φ) · θ/α` (Delaunay, a band's triangles stopping at its inner edge), the
/// sphere's on the sphere (their convex hull).
enum Triangulation {
    Disc {
        axis: Vec3,
        u: Vec3,
        v: Vec3,
        half_angle: f32,
        points: Vec<Vec2>,
        triangles: Vec<[usize; 3]>,
        boundary: Vec<(usize, usize)>,
        /// The farthest point's distance from the disc's centre: no triangle reaches past it.
        outer: f32,
    },
    Sphere {
        faces: Vec<[usize; 3]>,
    },
}

/// Up to three record directions about a direction and their weights, summing to 1.
#[derive(Clone, Copy, Debug)]
struct Corners {
    index: [usize; 3],
    weight: Vec3,
}

impl<'a> Samples<'a> {
    fn of(
        map: &Map,
        origin: Vec3,
        strength: f32,
        rays: &[Ray],
        bodies: Option<&'a HardBodies>,
    ) -> Self {
        let directions: Vec<Vec3> = rays.iter().map(|ray| ray.direction).collect();
        let values = rays
            .iter()
            .map(|ray| Vec2::new(ray.reach, ray.weight * strength))
            .collect();
        let mesh = if map.whole_sphere() {
            Triangulation::Sphere {
                faces: sphere_faces(&directions, map.axis),
            }
        } else {
            let points: Vec<Vec2> = directions
                .iter()
                .map(|&d| disc_point(map.axis, map.u, map.v, map.half_angle, d))
                .collect();
            let hole = map.inner / map.half_angle.max(1e-3);
            let mut triangles = delaunay(&points);
            triangles
                .retain(|t| (points[t[0]] + points[t[1]] + points[t[2]]).length() >= 3.0 * hole);
            let boundary = hull_edges(&triangles);
            let outer = points.iter().map(|p| p.length()).fold(0.0, f32::max) + 1e-5;
            Triangulation::Disc {
                axis: map.axis,
                u: map.u,
                v: map.v,
                half_angle: map.half_angle,
                points,
                triangles,
                boundary,
                outer,
            }
        };
        let mut incident = vec![Vec::new(); directions.len()];
        let triangles = match &mesh {
            Triangulation::Disc { triangles, .. } => triangles,
            Triangulation::Sphere { faces } => faces,
        };
        for (t, corners) in triangles.iter().enumerate() {
            for &corner in corners {
                incident[corner].push(t);
            }
        }
        let endings = match bodies {
            Some(bodies) => rays
                .iter()
                .map(
                    |ray| match bodies.first(origin, ray.direction, GUIDE_REACH_M) {
                        None => Ending::Free,
                        Some(hit) => {
                            let within = ray.reach >= hit.entry - ENDING_SLACK_M
                                && ray.reach <= hit.exit + ENDING_SLACK_M;
                            if within {
                                Ending::Within {
                                    body: hit.body,
                                    depth: ray.reach - hit.entry,
                                }
                            } else {
                                Ending::Past { body: hit.body }
                            }
                        }
                    },
                )
                .collect(),
            None => Vec::new(),
        };
        Self {
            origin,
            directions,
            values,
            mesh,
            incident,
            endings,
            bodies,
        }
    }

    /// The `(reach m, strength ref-mm)` along unit `direction`. The strength is the record's
    /// interpolation between the directions about it ([`Self::locate`]); so is the reach where
    /// those directions ended alike (within one body, past one, or free of every body), the
    /// record's own agreement, which the guide never overrides. Where they ended unlike, a shadow's
    /// edge lies between them, and the guide places it: the reach is the interpolation of the
    /// directions that ended like this one would, as the guide sees it (the same first body, or
    /// none), each a direction that ended within that body carrying its depth past the body's
    /// entry here, and every other its own reach; with none, the record's interpolation. So at a
    /// record direction the reach is the record's, a direction that passed a body included.
    fn at(&self, direction: Vec3, cache: &mut usize) -> Vec2 {
        let Some(corners) = self.locate(direction, cache) else {
            return Vec2::ZERO;
        };
        let blend = corners
            .index
            .iter()
            .zip(corners.weight.to_array())
            .fold(Vec2::ZERO, |sum, (&i, w)| sum + self.values[i] * w);
        let Some(bodies) = self.bodies else {
            return blend;
        };
        let weighed: Vec<usize> = (0..3).filter(|&k| corners.weight[k] > 0.0).collect();
        let first = self.endings[corners.index[weighed[0]]];
        if weighed
            .iter()
            .all(|&k| self.endings[corners.index[k]].alike(first))
        {
            return blend;
        }
        let hit = bodies.entry(self.origin, direction, GUIDE_REACH_M);
        let (mut sum, mut total) = (0.0, 0.0);
        for &k in &weighed {
            let i = corners.index[k];
            let ending = self.endings[i];
            if ending.body() != hit.map(|(_, body)| body) {
                continue;
            }
            let reach = match (ending, hit) {
                (Ending::Within { depth, .. }, Some((entry, _))) => entry + depth,
                _ => self.values[i].x,
            };
            sum += corners.weight[k] * reach;
            total += corners.weight[k];
        }
        let reach = if total > 0.0 { sum / total } else { blend.x };
        Vec2::new(reach, blend.y)
    }

    /// The triangles to try for a point after one in triangle `last`: that one, then those
    /// sharing a corner with it, then every one.
    fn candidates(&self, last: usize, count: usize) -> impl Iterator<Item = usize> + '_ {
        let near = match &self.mesh {
            Triangulation::Disc { triangles, .. } => triangles.get(last),
            Triangulation::Sphere { faces } => faces.get(last),
        };
        let around = near.into_iter().flat_map(|corners| {
            corners
                .iter()
                .flat_map(|&c| self.incident[c].iter().copied())
        });
        std::iter::once(last)
            .chain(around)
            .chain(0..count)
            .filter(move |&t| t < count)
    }

    /// The record directions about unit `direction` and their weights: on the disc, barycentric in
    /// the triangle that holds its disc point, else at the nearest point of the boundary, else
    /// (no triangle) the nearest direction alone; on the sphere, barycentric across the hull face
    /// whose cone holds it, at the point where it meets the face's plane, else the face it lies
    /// least outside, its weights held non-negative, else the nearest direction. `cache` holds the
    /// triangle that held the last direction, where the search starts. `None` with no directions.
    fn locate(&self, direction: Vec3, cache: &mut usize) -> Option<Corners> {
        const INSIDE: f32 = -1e-5;
        if self.directions.is_empty() {
            return None;
        }
        match &self.mesh {
            Triangulation::Disc {
                axis,
                u,
                v,
                half_angle,
                points,
                triangles,
                boundary,
                outer,
            } => {
                let q = disc_point(*axis, *u, *v, *half_angle, direction);
                let weights = |[a, b, c]: [usize; 3]| {
                    let (pa, pb, pc) = (points[a], points[b], points[c]);
                    let area = (pb - pa).perp_dot(pc - pa);
                    if area.abs() < 1e-12 {
                        return None;
                    }
                    let wb = (q - pa).perp_dot(pc - pa) / area;
                    let wc = (pb - pa).perp_dot(q - pa) / area;
                    let w = Vec3::new(1.0 - wb - wc, wb, wc);
                    (w.min_element() >= INSIDE)
                        .then(|| w.max(Vec3::ZERO) / w.max(Vec3::ZERO).element_sum())
                };
                if q.length() <= *outer {
                    for t in self.candidates(*cache, triangles.len()) {
                        if let Some(weight) = weights(triangles[t]) {
                            *cache = t;
                            return Some(Corners {
                                index: triangles[t],
                                weight,
                            });
                        }
                    }
                }
                let mut nearest: Option<(f32, Corners)> = None;
                for &(a, b) in boundary {
                    let span = points[b] - points[a];
                    let t = ((q - points[a]).dot(span) / span.length_squared().max(1e-12))
                        .clamp(0.0, 1.0);
                    let distance = q.distance(points[a] + span * t);
                    if nearest.is_none_or(|(best, _)| distance < best) {
                        nearest = Some((
                            distance,
                            Corners {
                                index: [a, b, a],
                                weight: Vec3::new(1.0 - t, t, 0.0),
                            },
                        ));
                    }
                }
                if boundary.is_empty() {
                    nearest = points
                        .iter()
                        .enumerate()
                        .map(|(k, p)| (q.distance(*p), k))
                        .min_by(|a, b| a.0.total_cmp(&b.0))
                        .map(|(distance, k)| {
                            (
                                distance,
                                Corners {
                                    index: [k; 3],
                                    weight: Vec3::X,
                                },
                            )
                        });
                }
                nearest.map(|(_, corners)| corners)
            }
            Triangulation::Sphere { faces } => {
                let d = &self.directions;
                let weights = |[a, b, c]: [usize; 3]| {
                    let (da, db, dc) = (d[a], d[b], d[c]);
                    let w = Vec3::new(
                        direction.dot(db.cross(dc)),
                        da.dot(direction.cross(dc)),
                        da.dot(db.cross(direction)),
                    );
                    let sum = w.element_sum();
                    (sum > 0.0).then(|| w / sum)
                };
                let held = |f: usize, weight: Vec3| {
                    let held = weight.max(Vec3::ZERO);
                    Corners {
                        index: faces[f],
                        weight: held / held.element_sum(),
                    }
                };
                for f in self
                    .candidates(*cache, faces.len())
                    .take(faces.len() / 2 + 8)
                {
                    if let Some(weight) = weights(faces[f])
                        && weight.min_element() >= INSIDE
                    {
                        *cache = f;
                        return Some(held(f, weight));
                    }
                }
                let mut best: Option<(f32, usize, Vec3)> = None;
                for (f, &face) in faces.iter().enumerate() {
                    let Some(weight) = weights(face) else {
                        continue;
                    };
                    let low = weight.min_element();
                    if best.is_none_or(|(have, ..)| low > have) {
                        best = Some((low, f, weight));
                    }
                    if low >= INSIDE {
                        break;
                    }
                }
                if let Some((_, f, weight)) = best {
                    *cache = f;
                    return Some(held(f, weight));
                }
                d.iter()
                    .enumerate()
                    .max_by(|a, b| a.1.dot(direction).total_cmp(&b.1.dot(direction)))
                    .map(|(k, _)| Corners {
                        index: [k; 3],
                        weight: Vec3::X,
                    })
            }
        }
    }
}

/// The polar disc point of unit `direction` about unit `axis` (`u`, `v` across it) for a support
/// out to `half_angle`: `(cos φ, sin φ) · θ/α`, azimuth from `u` toward `v`; the antipode, which
/// has no azimuth, at `(π/α, 0)`.
fn disc_point(axis: Vec3, u: Vec3, v: Vec3, half_angle: f32, direction: Vec3) -> Vec2 {
    let theta = direction.dot(axis).clamp(-1.0, 1.0).acos();
    let across = Vec2::new(direction.dot(u), direction.dot(v));
    across.normalize_or(Vec2::X) * (theta / half_angle.max(1e-3))
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

/// Delaunay triangles of `points` (Bowyer–Watson), as index triples. Fewer than three points give
/// none.
fn delaunay(points: &[Vec2]) -> Vec<[usize; 3]> {
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
    use std::f32::consts::TAU;

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

    /// The fixtures' axis and its frame, the map's own.
    const AXIS: Vec3 = Vec3::NEG_Z;

    /// The unit direction `theta` (rad) off [`AXIS`] at azimuth `phi` from its frame's `u`.
    fn direction(theta: f32, phi: f32) -> Vec3 {
        let (u, v) = AXIS.any_orthonormal_pair();
        AXIS * theta.cos() + (u * phi.cos() + v * phi.sin()) * theta.sin()
    }

    /// A fixture field of `n` directions about [`AXIS`] over the support from `inner` to
    /// `half_angle` (rad), strength 10 ref-mm, in equal-solid-angle strata (`cos θ` even across
    /// the support, the azimuth turning by the golden angle), direction `k` reaching
    /// `reach(k, direction)`, thrown with strength fraction `weight(direction)`. Fixture data.
    pub(in super::super) fn fixture_field(
        half_angle: f32,
        inner: f32,
        n: usize,
        reach: impl Fn(usize, Vec3) -> f32,
        weight: impl Fn(Vec3) -> f32,
    ) -> Cone {
        let golden = PI * (3.0 - 5.0_f32.sqrt());
        let (near, far) = (inner.cos(), half_angle.cos());
        let rays = (0..n)
            .map(|k| {
                let c = near + (far - near) * (k as f32 + 0.5) / n as f32;
                let d = direction(c.clamp(-1.0, 1.0).acos(), k as f32 * golden);
                Ray {
                    direction: d,
                    reach: reach(k, d),
                    weight: weight(d),
                }
            })
            .collect();
        Cone::new(Vec3::ZERO, AXIS, half_angle, inner, 10.0, AT_ORIGIN, rays)
    }

    /// A spall cone's cap, 25°, over 64 directions. Fixture data.
    pub(in super::super) fn cap(
        reach: impl Fn(usize, Vec3) -> f32,
        weight: impl Fn(Vec3) -> f32,
    ) -> Cone {
        fixture_field(25.0_f32.to_radians(), 0.0, 64, reach, weight)
    }

    /// A ring's band, 56° to 76° off the axis (a ring of `λ = 0.45` and half-width 10°), over
    /// 128 directions. Fixture data.
    pub(in super::super) fn band(reach: impl Fn(usize, Vec3) -> f32) -> Cone {
        fixture_field(
            76.0_f32.to_radians(),
            56.0_f32.to_radians(),
            128,
            reach,
            |_| 0.5,
        )
    }

    /// The whole sphere over 128 directions. Fixture data.
    pub(in super::super) fn sphere(reach: impl Fn(usize, Vec3) -> f32) -> Cone {
        fixture_field(PI, 0.0, 128, reach, |_| 0.5)
    }

    /// The map reads the record: a reach and a strength linear in the direction come back from a
    /// bilinear read of the map at every direction well inside the directions' hull, and the read
    /// at a texel's centre is that texel. The volume reads the record through it; a map read on
    /// swapped axes, a wrong barycentric weight, a misplaced bilinear corner or a gutter off by a
    /// texel fails here. Fixture: a cap of 64 directions; directions within 18° of its 25° axis
    /// lie inside their hull; tolerance the bilinear read's curvature error over a 0.24° texel
    /// (DERIVED, under 1e-4 of a unit-slope function).
    #[test]
    fn the_map_reproduces_what_is_linear_in_it() {
        let reach = |d: Vec3| 2.0 + 0.6 * d.x - 0.4 * d.y;
        let weight = |d: Vec3| 0.5 + 0.2 * d.y;
        let cone = cap(|_, d| reach(d), weight);
        let map = Map::of(&cone, None);
        let mut checked = 0;
        for k in 0..18 {
            for j in 0..12 {
                let d = direction((k as f32).to_radians(), j as f32 * TAU / 12.0);
                let got = map.sample(d);
                let want = Vec2::new(reach(d), 10.0 * weight(d));
                assert!(got.distance(want) < 2e-3, "{d}: {got} against {want}");
                checked += 1;
            }
        }
        for (i, j) in [(1, 1), (129, 77), (200, 129)] {
            let d = map.texel_direction(i, j);
            assert!(map.sample(d).distance(map.texels[j * MAP_TEXELS + i]) < 1e-4);
            assert!(map.near_support(d) || map.texels[j * MAP_TEXELS + i] == Vec2::ZERO);
        }
        assert!(checked > 200);
    }

    /// A band holds nothing off it: the map reads zero along every direction short of its inner
    /// edge or past its rim and reads the band on it. The look's volume tests the support before
    /// it reads the map (`pen_cam_look.wgsl`, `field_at`); a band drawn as the cap it lies in
    /// fills the cone about the line, where the ring threw nothing. Directions sampled half a
    /// degree off each edge, against an f32 angle good to 1e-5 rad.
    #[test]
    fn a_band_holds_nothing_off_it() {
        let cone = band(|_, _| 1.0);
        let map = Map::of(&cone, None);
        for k in 0..180 {
            let theta = (k as f32 + 0.5).to_radians();
            for j in 0..8 {
                let value = map.at(direction(theta, j as f32 * TAU / 8.0));
                if theta < map.inner || theta > map.half_angle {
                    assert_eq!(value, Vec2::ZERO, "{k}.5°");
                } else {
                    assert!((value.x - 1.0).abs() < 1e-4, "{k}.5°: {value}");
                }
            }
        }
    }

    /// A band's triangulation stops at its inner edge: every texel the map bakes inside the hole,
    /// which a read at the band's edge blends, holds the value of the band's nearest edge, never a
    /// blend across the hole of the band's two sides. A triangle spanning the hole carries the far
    /// side's reach toward the near edge. Fixture: reach 3 on the band's `+u` side, 1 on its `−u`
    /// side; the texels within 60° of either side's middle.
    #[test]
    fn a_band_is_not_interpolated_across_its_hole() {
        let (u, v) = AXIS.any_orthonormal_pair();
        let cone = band(|_, d| if d.dot(u) > 0.0 { 3.0 } else { 1.0 });
        let map = Map::of(&cone, None);
        let mut checked = 0;
        for j in 0..MAP_TEXELS {
            for i in 0..MAP_TEXELS {
                let d = map.texel_direction(i, j);
                if d.dot(AXIS).acos() >= map.inner || !map.near_support(d) {
                    continue;
                }
                let from_u = d.dot(v).abs().atan2(d.dot(u)).to_degrees();
                let want = if from_u < 60.0 {
                    3.0
                } else if from_u > 120.0 {
                    1.0
                } else {
                    continue;
                };
                let got = map.texels[j * MAP_TEXELS + i].x;
                assert!(
                    (got - want).abs() < 1e-3,
                    "({i}, {j}): {got} against {want}"
                );
                checked += 1;
            }
        }
        assert!(checked > 20, "{checked}");
    }

    /// The sphere's map has no seam and no pinch: across the octahedron's fold, where the map's
    /// edge wraps onto itself, and about the antipode, at the square's four corners, a read on
    /// either side of the fold or at any azimuth close about the antipode takes nearly the same
    /// value, and the antipode the value of the directions about it. The look reads the map
    /// bilinearly at the fold, so a gutter that does not wrap draws a seam of different reaches
    /// along the sphere's back. Fixture: a reach turning with azimuth, `2 + 0.5 d·u + 0.3 d·axis`;
    /// tolerances the hull faces' planes, `0.3 (1 − cos 0.6)` = 0.05 (DERIVED: faces twice the
    /// strata's spacing `√(4π / 128)` = 0.31 rad), and the reads 0.4° apart across the fold.
    #[test]
    fn the_sphere_has_no_seam_and_no_pinch() {
        let (u, _) = AXIS.any_orthonormal_pair();
        let reach = |d: Vec3| 2.0 + 0.5 * d.dot(u) + 0.3 * d.dot(AXIS);
        let cone = sphere(|_, d| reach(d));
        let map = Map::of(&cone, None);
        assert_eq!(map.zoom, 1.0);
        let antipode = map.sample(-AXIS).x;
        assert!((antipode - reach(-AXIS)).abs() < 0.05, "{antipode}");
        for j in 0..24 {
            let d = direction(PI - 0.02, j as f32 * TAU / 24.0);
            let got = map.sample(d).x;
            assert!(
                (got - antipode).abs() < 0.02,
                "{d}: {got} against {antipode}"
            );
        }
        // Across the fold: the lower hemisphere's edge `|x| + |y| = 1` of the square, read a fifth
        // of a degree either side of the equator's fold line `x = 1`.
        for k in 1..20 {
            let y = -0.95 + 0.1 * k as f32;
            let at = |x: f32| {
                let local = unfold(wrap(Vec2::new(x, y)));
                let (u, v) = AXIS.any_orthonormal_pair();
                map.sample(u * local.x + v * local.y + AXIS * local.z).x
            };
            let (inside, past) = (at(1.0 - 0.003), at(1.0 + 0.003));
            assert!(
                (inside - past).abs() < 0.01,
                "y = {y}: {inside} against {past}"
            );
        }
    }

    /// A box of hard body `body` from `lo` to `hi`: 12 triangles wound outward.
    fn cuboid(lo: Vec3, hi: Vec3, body: u32, bodies: &mut (Vec<Vec3>, Vec<[u32; 3]>, Vec<u32>)) {
        let base = bodies.0.len() as u32;
        let centre = 0.5 * (lo + hi);
        for k in 0..8 {
            bodies.0.push(Vec3::new(
                if k & 1 == 0 { lo.x } else { hi.x },
                if k & 2 == 0 { lo.y } else { hi.y },
                if k & 4 == 0 { lo.z } else { hi.z },
            ));
        }
        let faces = [
            [0, 1, 3, 2],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 3, 7, 5],
        ];
        for [a, b, c, d] in faces {
            for [x, y, z] in [[a, b, c], [a, c, d]] {
                let p = |i: u32| bodies.0[(base + i) as usize];
                let normal = (p(y) - p(x)).cross(p(z) - p(x));
                let out = normal.dot((p(x) + p(y) + p(z)) / 3.0 - centre) > 0.0;
                let t = if out { [x, y, z] } else { [x, z, y] };
                bodies.1.push(t.map(|i| base + i));
                bodies.2.push(body);
            }
        }
    }

    /// Fixture bodies about a cap down [`AXIS`] from the origin: a back wall 3 m down (body 0), a
    /// plate 1 m down covering the cap's `+u` half (body 1), and a rod 2 m down on the `−u` side,
    /// 2 cm across along `v` (body 2). Fixture data.
    fn fixture_bodies() -> HardBodies {
        let (u, v) = AXIS.any_orthonormal_pair();
        assert_eq!(
            (u, v),
            (Vec3::X, Vec3::NEG_Y),
            "the fixture's sides are its frame's"
        );
        let mut parts = (Vec::new(), Vec::new(), Vec::new());
        cuboid(
            Vec3::new(-5.0, -5.0, -3.2),
            Vec3::new(5.0, 5.0, -3.0),
            0,
            &mut parts,
        );
        cuboid(
            Vec3::new(0.0, -5.0, -1.1),
            Vec3::new(5.0, 5.0, -1.0),
            1,
            &mut parts,
        );
        cuboid(
            Vec3::new(-0.4, -0.01, -2.1),
            Vec3::new(-0.3, 0.01, -2.0),
            2,
            &mut parts,
        );
        HardBodies::new(parts.0, parts.1, parts.2)
    }

    /// The guide's own reach along `d`: the first body's entry.
    fn guide(bodies: &HardBodies, d: Vec3) -> f32 {
        bodies.entry(Vec3::ZERO, d, GUIDE_REACH_M).unwrap().0
    }

    /// Where the record's directions about a point ended unlike, the shadow's edge is the body's
    /// silhouette, not the record's interpolation: every texel of a cap whose record directions
    /// each end where the bodies stop them reads the guide's reach, the plate's or the wall's, up
    /// to a texel from the plate's edge, while the record's interpolation alone smears the edge
    /// over the ~5° between directions. The look draws every shadow from the map; a map that
    /// interpolates across a silhouette draws the wavy blobs this replaces. Fixture: the plate and
    /// the wall of [`fixture_bodies`], the rod's side away from the read.
    #[test]
    fn a_shadow_edge_is_the_bodys_silhouette() {
        let bodies = fixture_bodies();
        let cone = cap(|_, d| guide(&bodies, d), |_| 0.5);
        let guided = Map::of(&cone, Some(&bodies));
        let raw = Map::of(&cone, None);
        let texel = 4.0 * guided.zoom / MAP_SIDE as f32;
        let (mut checked, mut smeared) = (0, 0);
        for k in 1..60 {
            // Directions across the plate's edge (`d · u = 0`), on the `+v` side, off the rod.
            let off = (k as f32 - 30.0) * 0.1_f32.to_radians();
            for theta in [8.0_f32, 14.0, 20.0] {
                let d = direction(
                    theta.to_radians(),
                    FRAC_PI_2 + off / theta.to_radians().sin(),
                );
                let edge = d.x.abs() / d.dot(AXIS);
                if edge < 1.5 * texel {
                    continue;
                }
                let want = guide(&bodies, d);
                let got = guided.sample(d).x;
                assert!((got - want).abs() < 0.01, "{d}: {got} against {want}");
                smeared += usize::from((raw.sample(d).x - want).abs() > 0.1);
                checked += 1;
            }
        }
        assert!(checked > 100 && smeared > 10, "{checked} {smeared}");
    }

    /// Where every record direction about a point ended alike, the record decides, never the
    /// guide: a rod 2 cm across that no record direction meets, lying between directions that
    /// all ended on the wall, leaves the map reading the wall behind it, as the record does. The
    /// record is the outcome; a body the sim's directions missed is no shadow in it. Fixture: the
    /// rod of [`fixture_bodies`], read along its middle.
    #[test]
    fn agreeing_directions_are_never_overridden() {
        let bodies = fixture_bodies();
        let cone = cap(|_, d| guide(&bodies, d), |_| 0.5);
        let hits_rod = |d: Vec3| bodies.entry(Vec3::ZERO, d, GUIDE_REACH_M).unwrap().1 == 2;
        assert!(cone.rays.iter().all(|ray| !hits_rod(ray.direction)));
        let map = Map::of(&cone, Some(&bodies));
        let mut checked = 0;
        for k in 0..9 {
            let d = Vec3::new(-0.3 - 0.0125 * k as f32 - 0.0025, 0.0, -2.05).normalize();
            assert!(hits_rod(d));
            let got = map.sample(d).x;
            let wall = 3.0 / d.dot(AXIS);
            assert!(
                (got - wall).abs() < 0.01,
                "{d}: {got} against the wall's {wall}"
            );
            checked += 1;
        }
        assert_eq!(checked, 9);
    }

    /// A direction the record says passed a body the guide would stop at keeps the record's
    /// reach: the record is the outcome, and a fragment the sim let through a body ends where the
    /// sim ended it. Fixture: one direction under the plate recorded on to the wall, its
    /// neighbours stopped at the plate.
    #[test]
    fn a_passed_body_keeps_the_records_reach() {
        let bodies = fixture_bodies();
        let through = cap(|_, _| 1.0, |_| 0.5)
            .rays
            .iter()
            .position(|ray| ray.direction.x > 0.05)
            .expect("a direction under the plate");
        let cone = cap(
            |k, d| {
                if k == through {
                    3.0 / d.dot(AXIS)
                } else {
                    guide(&bodies, d)
                }
            },
            |_| 0.5,
        );
        let ray = &cone.rays[through];
        let map = Map::of(&cone, Some(&bodies));
        let samples = Samples::of(&map, Vec3::ZERO, cone.strength, &cone.rays, Some(&bodies));
        assert_eq!(samples.endings[through], Ending::Past { body: 1 });
        let got = samples.at(ray.direction, &mut 0).x;
        assert!(
            (got - ray.reach).abs() < 1e-4,
            "{got} against {}",
            ray.reach
        );
    }

    /// A field with no directions bakes to zero: nothing is drawn of it.
    #[test]
    fn a_field_with_no_directions_bakes_to_zero() {
        let cone = Cone::new(Vec3::ZERO, AXIS, 0.4, 0.0, 10.0, AT_ORIGIN, Vec::new());
        let map = Map::of(&cone, Some(&fixture_bodies()));
        assert!(map.texels.iter().all(|t| *t == Vec2::ZERO));
    }

    /// The atlas holds each field's map in its own tile, texel for texel as half floats, and
    /// nothing else: the look's shader reads field `k` at [`tile`]`(k)`. A tile placed apart from
    /// where the shader reads it draws one field's shadows in another.
    #[test]
    fn the_atlas_holds_each_map_in_its_tile() {
        let cones = [cap(|k, _| 1.0 + 0.01 * k as f32, |_| 0.5), band(|_, _| 2.0)];
        let maps = bake(&cones, None);
        let image = atlas(&maps);
        assert_eq!(image.texture_descriptor.format, TextureFormat::Rg16Float);
        assert_eq!(image.width() as usize, 2 * MAP_TEXELS);
        for (k, map) in maps.iter().enumerate() {
            for (i, j) in [(0, 0), (130, 140), (MAP_TEXELS - 1, MAP_TEXELS - 1)] {
                let at = tile(k) + UVec2::new(i as u32, j as u32);
                let texel = image.get_color_at(at.x, at.y).unwrap().to_linear();
                let want = map.texels[j * MAP_TEXELS + i];
                let got = Vec2::new(texel.red, texel.green);
                assert!(
                    got.distance(want) < 2e-3 * want.length().max(1.0),
                    "{k} {i} {j}"
                );
            }
        }
        let empty = tile(3);
        assert_eq!(
            image
                .get_color_at(empty.x + 9, empty.y + 9)
                .unwrap()
                .to_linear()
                .red,
            0.0
        );
    }
}
