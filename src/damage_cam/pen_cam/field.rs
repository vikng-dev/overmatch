//! A field's recorded directions as one function of direction, baked onto a direction map. A field
//! is a spall cone or a burst's; its support is a cap about its axis out to its half-angle α, a
//! ring's band from its inner angle out to α, or, for a ball whose rim is π, the whole sphere.
//!
//! The record samples the field at its directions, each with the reach the record gives it. Between
//! them the reach is the record's linear interpolation wherever the directions about a point ended
//! alike: inside one hard body, past one, or free of every body. Where they ended unlike, a shadow's edge lies between them, and the struck tank's
//! hard bodies ([`HardBodies`], every non-witness ballistic primitive, posed as the record has the
//! tank) place it: the reach follows the directions that ended as the point's own line would.
//! [`Samples::at`] states that contract; the design narrative is design doc §5, "Potency colour"
//! (`.agents/docs/design/armor-penetration-and-damage.md`). The map holds the reach alone: the
//! strength along a direction is the field's law (`look::law`), not the record's interpolation.
//!
//! A cap's and a band's directions are triangulated on the polar disc `q = (cos φ, sin φ) · θ/α`
//! (Delaunay), a band's triangles stopping at its inner edge, none spanning the hole; the sphere's
//! on the sphere, by their convex hull, so its interpolation has no rim and no seam.
//!
//! [`Map`] bakes the function onto an octahedral map about the field's axis, [`MAP_TEXELS`]² texels
//! with a one-texel gutter; [`atlas`] packs a record's maps into the one texture the look's shader
//! reads them from (`pen_cam_look.wgsl`, `map_at`). The look tests a point against each of the
//! four texels about its direction, each against its own reach within a tolerance
//! ([`REACH_TOLERANCE_DIAGONALS`]), and weighs the results bilinearly, so a reach is never
//! interpolated across a shadow's edge; [`Map::lit`] is the same read on the CPU, and
//! [`Map::sample`] the bilinear blend of the four, the map's own content between texels. [`bake`]
//! bakes every field of a record, its rows spread over the machine's threads.

use std::f32::consts::{FRAC_PI_2, PI, SQRT_2};
use std::sync::Mutex;

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
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
/// map's edge (across the octahedron's fold on the sphere), so a read at the edge weighs what lies
/// across it.
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
/// `pen_cam_look.wgsl` declares it (`map_at`).
pub const OCTAHEDRAL_TURN: f32 = 3.0;
/// The texel diagonals past the support's edges a map bakes, each [`Map::diagonal`] rad: a read on
/// the support weighs texels at most one diagonal off it, and the second keeps whole a read up to
/// a diagonal past an edge, where a painted surface's edge a pixel wide reads
/// (`pen_cam_look.wgsl`, `footprint`). Every texel farther off holds zero.
const SUPPORT_MARGIN_DIAGONALS: f32 = 2.0;
/// How far past a texel's own reach the look counts a point within it, at most: this many texel
/// diagonals of turn ([`Map::diagonal`]) times the point's distance from the origin, and never
/// past the farthest reach of the four texels about the point's direction ([`Map::lit`]). TUNED.
///
/// A hard body stops the field on its surface, so each texel's reach lands on the surface along
/// the texel's own direction, and a surface point between texel directions lies past the reach of
/// those whose directions meet the surface nearer: read without a tolerance, the last stretch
/// before the surface flips texel by texel, a stair-step that stretches into streaks where the
/// surface turns edge-on to the field. Each texel's reach lies at or past its direction's entry
/// into the body, and a direction between the four enters a convex body no farther than the
/// farthest of theirs (the directions that meet it within a distance form a convex set), so with
/// the tolerance a surface point reads inside wherever the reach grows by less than the tolerance
/// across the four: where the turn is steepest
/// (`OCTAHEDRAL_TURN`), a surface up to `tan⁻¹ 2` = 63° off square to the field (DERIVED), and
/// steeper where the turn is less, up to 80° at a cap's axis (DERIVED, a turn of 1). Where the four
/// texels all end on a blocker, none reads past the farthest of them, so a point behind it stays
/// outside however near; across a silhouette a blocked texel reads past its blocker only for a
/// surface within the tolerance behind it, 2.2 cm at 1 m for a 26° spall cone and 6.6 cm for the
/// whole sphere (DERIVED), and the shadow's edge there moves at most the one texel cell the
/// silhouette crosses (DERIVED); on a surface farther behind, not at all.
pub const REACH_TOLERANCE_DIAGONALS: f32 = 2.0;

/// A field's recorded directions as one function of direction, baked onto an octahedral map about
/// the field's axis: [`MAP_TEXELS`]² texels, row by row, each the reach (m) the record gives along
/// the texel's direction ([`Samples::at`]). The map covers the square of
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
    pub texels: Vec<f32>,
    /// The longest reach any texel holds (m): no direction of the field reaches past it.
    pub longest: f32,
}

impl Map {
    /// `cone`'s map, its shadows' edges placed between its directions by `bodies`, the struck
    /// tank's hard bodies in the record's frame, where the record's directions leave them open;
    /// without bodies, the record's interpolation alone.
    pub fn of(cone: &Cone, bodies: Option<&HardBodies>) -> Self {
        let (u, v) = cone.axis.any_orthonormal_pair();
        let zoom = if cone.half_angle() >= FRAC_PI_2 {
            1.0
        } else {
            let (s, c) = cone.half_angle().sin_cos();
            s / (s + c)
        };
        let mut map = Self {
            axis: cone.axis,
            u,
            v,
            half_angle: cone.half_angle(),
            inner: cone.inner(),
            zoom,
            texels: Vec::new(),
            longest: 0.0,
        };
        let samples = Samples::of(&map, cone.origin, &cone.rays, bodies);
        let mut texels = vec![0.0; MAP_TEXELS * MAP_TEXELS];
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
        map.longest = texels.iter().copied().fold(0.0, f32::max);
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

    /// The most a direction turns (rad) across one texel's diagonal:
    /// `OCTAHEDRAL_TURN · √2 · 2 · zoom / MAP_SIDE` (DERIVED).
    pub fn diagonal(&self) -> f32 {
        OCTAHEDRAL_TURN * SQRT_2 * 2.0 * self.zoom / MAP_SIDE as f32
    }

    /// Unit `direction` lies within [`SUPPORT_MARGIN_DIAGONALS`] of the support.
    fn near_support(&self, direction: Vec3) -> bool {
        let margin = SUPPORT_MARGIN_DIAGONALS * self.diagonal();
        let theta = direction.dot(self.axis).clamp(-1.0, 1.0).acos();
        theta >= self.inner - margin && theta <= self.half_angle + margin
    }

    /// The four texels about unit `direction`'s map coordinate, `(i, j)`, `(i + 1, j)`,
    /// `(i, j + 1)` and `(i + 1, j + 1)`, the clamp holding them inside the map, and its place
    /// between them (0..1 along `i` and `j`).
    fn footprint(&self, direction: Vec3) -> ([f32; 4], Vec2) {
        let top = (MAP_TEXELS - 1) as f32;
        let g = ((self.coordinate(direction) + 1.0) * 0.5 * MAP_SIDE as f32 + 0.5)
            .clamp(Vec2::ZERO, Vec2::splat(top));
        let low = g.floor().min(Vec2::splat(top - 1.0));
        let (i, j) = (low.x as usize, low.y as usize);
        let at = |i: usize, j: usize| self.texels[j * MAP_TEXELS + i];
        (
            [at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1)],
            g - low,
        )
    }

    /// The reach (m) along unit `direction`, bilinear between the four texels about its map
    /// coordinate: the map's content between texels, which the tests read. The look never blends a
    /// reach ([`Self::lit`]).
    pub fn sample(&self, direction: Vec3) -> f32 {
        let ([a, b, c, d], t) = self.footprint(direction);
        let (below, above) = (a + (b - a) * t.x, c + (d - c) * t.x);
        below + (above - below) * t.y
    }

    /// The share (0..1) of the field lit along unit `direction` at `r` world m from its origin,
    /// `scale` world m per record m: each of the four texels about its map coordinate lights where
    /// `r` lies within its own reach, and not past it, and the four are weighted bilinearly. A texel's reach counts out to its tolerance past it
    /// ([`REACH_TOLERANCE_DIAGONALS`]: up to that many [`Self::diagonal`]s times `r`, never past
    /// the farthest of the four), and `slack` world m past that. A reach is compared per texel and
    /// never interpolated, so across a silhouette a point behind the blocker, past the tolerance,
    /// gets the open texels' share alone, the same at every depth out to the wall, and a point on
    /// the open side some share right up to the edge. The look's shader reads the map so
    /// (`pen_cam_look.wgsl`, `map_at`); this is the read it mirrors, held by the tests.
    pub fn lit(&self, direction: Vec3, r: f32, scale: f32, slack: f32) -> f32 {
        let (texels, t) = self.footprint(direction);
        let reach = texels.map(|texel| texel * scale);
        let farthest = reach.into_iter().fold(0.0, f32::max);
        let tolerance = REACH_TOLERANCE_DIAGONALS * self.diagonal() * r;
        let [a, b, c, d] = std::array::from_fn(|k| {
            let met = farthest.min(reach[k] + tolerance) + slack;
            if r <= met { 1.0 } else { 0.0 }
        });
        let (below, above) = (a + (b - a) * t.x, c + (d - c) * t.x);
        below + (above - below) * t.y
    }

    /// The reach (m) along unit `direction`: [`Self::sample`] on the support, zero off it, short
    /// of the inner edge or past the rim.
    pub fn at(&self, direction: Vec3) -> f32 {
        let theta = direction.dot(self.axis).clamp(-1.0, 1.0).acos();
        if theta < self.inner || theta > self.half_angle {
            return 0.0;
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
/// texel's reach (m) as a full float, the value [`Map::lit`] reads, loaded unfiltered; an unused
/// tile holds zero.
pub fn atlas(maps: &[Map]) -> Image {
    let side = ATLAS_TILES * MAP_TEXELS;
    let mut reach = vec![0.0_f32; side * side];
    for (index, map) in maps.iter().take(MAP_FIELDS).enumerate() {
        let origin = tile(index);
        for (k, &texel) in map.texels.iter().enumerate() {
            let (x, y) = (
                origin.x as usize + k % MAP_TEXELS,
                origin.y as usize + k / MAP_TEXELS,
            );
            reach[y * side + x] = texel;
        }
    }
    Image::new(
        Extent3d {
            width: side as u32,
            height: side as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        reach.iter().flat_map(|texel| texel.to_le_bytes()).collect(),
        TextureFormat::R32Float,
        RenderAssetUsages::default(),
    )
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

/// A field's record as one function of direction: its directions, each one's reach (m), their
/// triangulation, and with the struck tank's hard bodies, how each ended.
struct Samples<'a> {
    origin: Vec3,
    directions: Vec<Vec3>,
    reaches: Vec<f32>,
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
    fn of(map: &Map, origin: Vec3, rays: &[Ray], bodies: Option<&'a HardBodies>) -> Self {
        let directions: Vec<Vec3> = rays.iter().map(|ray| ray.direction).collect();
        let reaches = rays.iter().map(|ray| ray.reach).collect();
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
            reaches,
            mesh,
            incident,
            endings,
            bodies,
        }
    }

    /// The reach (m) along unit `direction`: the record's interpolation between the directions
    /// about it ([`Self::locate`]) where those directions ended alike (within one body, past one, or free of every body), the
    /// record's own agreement, which the guide never overrides. Where they ended unlike, a shadow's
    /// edge lies between them, and the guide places it: the reach is the interpolation of the
    /// directions that ended like this one would, as the guide sees it (the same first body, or
    /// none), each a direction that ended within that body carrying its depth past the body's
    /// entry here, and every other its own reach; with none, the record's interpolation. So at a
    /// record direction the reach is the record's, a direction that passed a body included.
    fn at(&self, direction: Vec3, cache: &mut usize) -> f32 {
        let Some(corners) = self.locate(direction, cache) else {
            return 0.0;
        };
        let blend = corners
            .index
            .iter()
            .zip(corners.weight.to_array())
            .fold(0.0, |sum, (&i, w)| sum + self.reaches[i] * w);
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
                _ => self.reaches[i],
            };
            sum += corners.weight[k] * reach;
            total += corners.weight[k];
        }
        if total > 0.0 { sum / total } else { blend }
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
    use crate::ballistics::spall_cone::FieldDescriptor;

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

    /// A fixture field of law `field` over `n` directions about [`AXIS`], strength 10 ref-mm, in
    /// equal-solid-angle strata (`cos θ` even across the support, the azimuth turning by the
    /// golden angle), direction `k` reaching `reach(k, direction)`. Fixture data.
    pub(in super::super) fn fixture_field(
        field: FieldDescriptor,
        n: usize,
        reach: impl Fn(usize, Vec3) -> f32,
    ) -> Cone {
        let golden = PI * (3.0 - 5.0_f32.sqrt());
        let (near, far) = (field.inner_angle().cos(), field.half_angle().cos());
        let rays = (0..n)
            .map(|k| {
                let c = near + (far - near) * (k as f32 + 0.5) / n as f32;
                let d = direction(c.clamp(-1.0, 1.0).acos(), k as f32 * golden);
                Ray {
                    direction: d,
                    reach: reach(k, d),
                }
            })
            .collect();
        Cone::new(Vec3::ZERO, AXIS, field, n, 10.0, AT_ORIGIN, rays)
    }

    /// A spall cone's cap, 25° (a ball of `λ = 1 / sin 25°`), over 64 directions. Fixture data.
    pub(in super::super) fn cap(reach: impl Fn(usize, Vec3) -> f32) -> Cone {
        let ratio = 1.0 / 25.0_f32.to_radians().sin();
        fixture_field(FieldDescriptor::ball(ratio), 64, reach)
    }

    /// A ring's band of `λ = 0.45` and half-width 10°, 55.8° to 75.8° off the axis (DERIVED,
    /// `atan(1/λ) ∓ 10°`), over 128 directions. Fixture data.
    pub(in super::super) fn band(reach: impl Fn(usize, Vec3) -> f32) -> Cone {
        let field = FieldDescriptor::ring(0.45, 10.0_f32.to_radians());
        fixture_field(field, 128, reach)
    }

    /// The whole sphere (a ball of `λ = 0`) over 128 directions. Fixture data.
    pub(in super::super) fn sphere(reach: impl Fn(usize, Vec3) -> f32) -> Cone {
        fixture_field(FieldDescriptor::ball(0.0), 128, reach)
    }

    /// The map reads the record: a reach linear in the direction comes back from a bilinear read
    /// of the map at every direction well inside the directions' hull, and the read at a texel's
    /// centre is that texel. The volume reads the record through it; a map read on
    /// swapped axes, a wrong barycentric weight, a misplaced bilinear corner or a gutter off by a
    /// texel fails here. Fixture: a cap of 64 directions; directions within 18° of its 25° axis
    /// lie inside their hull; tolerance the bilinear read's curvature error over a 0.24° texel
    /// (DERIVED, under 1e-4 of a unit-slope function).
    #[test]
    fn the_map_reproduces_what_is_linear_in_it() {
        let reach = |d: Vec3| 2.0 + 0.6 * d.x - 0.4 * d.y;
        let cone = cap(|_, d| reach(d));
        let map = Map::of(&cone, None);
        let mut checked = 0;
        for k in 0..18 {
            for j in 0..12 {
                let d = direction((k as f32).to_radians(), j as f32 * TAU / 12.0);
                let (got, want) = (map.sample(d), reach(d));
                assert!((got - want).abs() < 2e-3, "{d}: {got} against {want}");
                checked += 1;
            }
        }
        for (i, j) in [(1, 1), (129, 77), (200, 129)] {
            let d = map.texel_direction(i, j);
            assert!((map.sample(d) - map.texels[j * MAP_TEXELS + i]).abs() < 1e-4);
            assert!(map.near_support(d) || map.texels[j * MAP_TEXELS + i] == 0.0);
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
                    assert_eq!(value, 0.0, "{k}.5°");
                } else {
                    assert!((value - 1.0).abs() < 1e-4, "{k}.5°: {value}");
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
                let got = map.texels[j * MAP_TEXELS + i];
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
    /// value, and the antipode the value of the directions about it. The look weighs the four
    /// texels about a direction at the fold too, so a gutter that does not wrap draws a seam of
    /// different reaches along the sphere's back. Fixture: a reach turning with azimuth,
    /// `2 + 0.5 d·u + 0.3 d·axis`; tolerances the hull faces' planes, `0.3 (1 − cos 0.6)` = 0.05
    /// (DERIVED: faces twice the strata's spacing `√(4π / 128)` = 0.31 rad), and the reads 0.4°
    /// apart across the fold.
    #[test]
    fn the_sphere_has_no_seam_and_no_pinch() {
        let (u, _) = AXIS.any_orthonormal_pair();
        let reach = |d: Vec3| 2.0 + 0.5 * d.dot(u) + 0.3 * d.dot(AXIS);
        let cone = sphere(|_, d| reach(d));
        let map = Map::of(&cone, None);
        assert_eq!(map.zoom, 1.0);
        let antipode = map.sample(-AXIS);
        assert!((antipode - reach(-AXIS)).abs() < 0.05, "{antipode}");
        for j in 0..24 {
            let d = direction(PI - 0.02, j as f32 * TAU / 24.0);
            let got = map.sample(d);
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
                map.sample(u * local.x + v * local.y + AXIS * local.z)
            };
            let (inside, past) = (at(1.0 - 0.003), at(1.0 + 0.003));
            assert!(
                (inside - past).abs() < 0.01,
                "y = {y}: {inside} against {past}"
            );
        }
    }

    /// A box of hard body `body` from `lo` to `hi`: 12 triangles wound outward.
    pub(in super::super) fn cuboid(
        lo: Vec3,
        hi: Vec3,
        body: u32,
        bodies: &mut (Vec<Vec3>, Vec<[u32; 3]>, Vec<u32>),
    ) {
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
    pub(in super::super) fn guide(bodies: &HardBodies, d: Vec3) -> f32 {
        bodies.entry(Vec3::ZERO, d, GUIDE_REACH_M).unwrap().0
    }

    /// Where the record's directions about a point ended unlike, the shadow's edge is the body's
    /// silhouette, not the record's interpolation: every texel of a cap whose record directions
    /// each end where the bodies stop them reads the guide's reach, the plate's or the wall's, up
    /// to a texel from the plate's edge, while the record's interpolation alone smears the edge
    /// over the ~5° between directions. And the look's read ([`Map::lit`]) never reaches a depth
    /// between the two sides, right up to the edge: a point between the plate and the wall reads
    /// the same share of the field at every depth there, behind the plate less than the whole and
    /// none once its texels are all the plate's, on the open side some and the whole once its
    /// texels are all the wall's. The look draws every shadow from the map; a map that
    /// interpolates across a silhouette draws the wavy blobs this replaces, and a read that blends
    /// the texels' reaches before testing a point against them carries the field past the plate
    /// to a depth no texel holds. Fixture: the plate and the wall of [`fixture_bodies`], the rod's
    /// side away from the read; points 1.2 and 2.9 times the plate's
    /// depth along a direction, past every plate texel's reach about it and its tolerance (under
    /// 3 cm there, [`REACH_TOLERANCE_DIAGONALS`]) and short of every wall texel's (DERIVED: the
    /// texels about a direction lie within half a degree of it).
    #[test]
    fn a_shadow_edge_is_the_bodys_silhouette() {
        let bodies = fixture_bodies();
        let cone = cap(|_, d| guide(&bodies, d));
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
                let got = guided.sample(d);
                assert!((got - want).abs() < 0.01, "{d}: {got} against {want}");
                smeared += usize::from((raw.sample(d) - want).abs() > 0.1);
                checked += 1;
            }
        }
        assert!(checked > 100 && smeared > 10, "{checked} {smeared}");
        // The look's read across the edge, the texels either side of it included: the whole field
        // lit is a share of 1.
        let full = 1.0;
        let (mut read, mut shared) = (0, 0);
        for k in 0..=200 {
            let off = (k as f32 - 100.0) * 0.01_f32.to_radians();
            for theta in [8.0_f32, 14.0, 20.0] {
                let d = direction(
                    theta.to_radians(),
                    FRAC_PI_2 + off / theta.to_radians().sin(),
                );
                let depth = d.dot(AXIS);
                let behind = guided.lit(d, 1.2 / depth, 1.0, 0.0);
                let before_wall = guided.lit(d, 2.9 / depth, 1.0, 0.0);
                assert!(
                    (behind - before_wall).abs() <= 1e-5 * full,
                    "{d}: {behind} behind the plate, {before_wall} before the wall"
                );
                let blocked = bodies.entry(Vec3::ZERO, d, GUIDE_REACH_M).unwrap().1 == 1;
                let clear = d.x.abs() / depth >= 1.5 * texel;
                if blocked {
                    assert!(
                        behind < full * (1.0 - 1e-4),
                        "{d}: {behind} behind the plate"
                    );
                    assert!(!clear || behind == 0.0, "{d}: {behind} behind the plate");
                } else {
                    assert!(before_wall > 0.0, "{d}: nothing on the open side");
                    assert!(
                        !clear || (before_wall - full).abs() <= 1e-4 * full,
                        "{d}: {before_wall} on the open side"
                    );
                }
                shared += usize::from(behind > 0.0 && behind < full * (1.0 - 1e-4));
                read += 1;
            }
        }
        assert!(read == 603 && shared > 20, "{read} {shared}");
    }

    /// A surface the field stops on reads inside across it: on a plane every direction of a cap
    /// stops on, each texel reaching the plane along its own direction, a point of the plane reads
    /// the field whole at every direction swept across it, though about most of them a texel
    /// whose direction meets the plane nearer leaves the point past that texel's own reach. The
    /// volume's march ends on a component's surface and the painting lies on it; a read with no
    /// tolerance ([`REACH_TOLERANCE_DIAGONALS`]) flips the last stretch before the surface texel by
    /// texel, a stair-step, and a tolerance not grown with the distance flips it on a surface far
    /// out. Fixture: the 25° cap; the plane 3 m out, turned 45° about `v`, so the swept directions,
    /// up to 20° off the axis, meet it from 20° to 60° off square and 3 to 6 m out, short of
    /// `tan⁻¹ 2` = 63°, the steepest the tolerance holds wherever the octahedral turn is steepest
    /// (DERIVED); every texel's reach the plane's distance along it, as the guide bakes a surface.
    #[test]
    fn a_surface_the_field_stops_on_reads_inside_across_it() {
        let (u, _) = AXIS.any_orthonormal_pair();
        let tilt = 45.0_f32.to_radians();
        // The plane's unit normal away from the origin, and its distance from it.
        let away = AXIS * tilt.cos() + u * tilt.sin();
        let distance = 3.0;
        let plane = |d: Vec3| distance / d.dot(away);
        let mut map = Map::of(&cap(|_, _| 1.0), None);
        for j in 0..MAP_TEXELS {
            for i in 0..MAP_TEXELS {
                let d = map.texel_direction(i, j);
                let texel = &mut map.texels[j * MAP_TEXELS + i];
                if *texel > 0.0 {
                    *texel = plane(d);
                }
            }
        }
        let full = 1.0;
        let (mut checked, mut stepped) = (0, 0);
        for k in 0..60 {
            for j in 0..48 {
                let d = direction((k as f32 / 3.0).to_radians(), j as f32 * TAU / 48.0 + 0.1);
                if d.dot(away).acos() > 60.0_f32.to_radians() {
                    continue;
                }
                let r = plane(d);
                let got = map.lit(d, r, 1.0, 0.0);
                assert!(
                    (got - full).abs() <= 1e-5 * full,
                    "{d} at {r} m: {got} of {full}"
                );
                let (texels, _) = map.footprint(d);
                stepped += usize::from(texels.iter().any(|&texel| texel < r));
                checked += 1;
            }
        }
        assert!(
            checked > 2000 && 2 * stepped > checked,
            "{checked} {stepped}"
        );
    }

    /// A point a body's thickness behind a blocker stays outside: behind a plate 3 cm thick 1 m
    /// out, a point on the plate's far face reads the field as one before the wall behind it does,
    /// the open texels' share alone, at every direction across the plate's silhouette; and 5 mm
    /// past the plate's near face, where every texel about it ends on the plate, a point reads
    /// nothing. A blocked texel reads past its reach by at most [`REACH_TOLERANCE_DIAGONALS`]
    /// diagonals times the distance (2.3 cm here, DERIVED) and never past the farthest of the four
    /// about a direction, so a shadow holds behind a body thicker than that; a tolerance wider (more
    /// diagonals, or a fixed length past the plate's thickness) carries the field onto the far
    /// face beside the silhouette, and one reaching past the farthest of the four carries it into
    /// the plate. Fixture: a wall 3 m out (body 0), the plate over the cap's `+u` half (body 1),
    /// the record's directions ending where they meet them.
    #[test]
    fn a_point_a_bodys_thickness_behind_a_blocker_stays_outside() {
        let thickness = 0.03;
        let mut parts = (Vec::new(), Vec::new(), Vec::new());
        cuboid(
            Vec3::new(-5.0, -5.0, -3.2),
            Vec3::new(5.0, 5.0, -3.0),
            0,
            &mut parts,
        );
        cuboid(
            Vec3::new(0.0, -5.0, -1.0 - thickness),
            Vec3::new(5.0, 5.0, -1.0),
            1,
            &mut parts,
        );
        let bodies = HardBodies::new(parts.0, parts.1, parts.2);
        let cone = cap(|_, d| guide(&bodies, d));
        let map = Map::of(&cone, Some(&bodies));
        let full = 1.0;
        let texel = 4.0 * map.zoom / MAP_SIDE as f32;
        let (mut across, mut within) = (0, 0);
        for k in 0..=200 {
            let off = (k as f32 - 100.0) * 0.01_f32.to_radians();
            for theta in [8.0_f32, 14.0, 20.0] {
                let d = direction(
                    theta.to_radians(),
                    FRAC_PI_2 + off / theta.to_radians().sin(),
                );
                if bodies.entry(Vec3::ZERO, d, GUIDE_REACH_M).unwrap().1 != 1 {
                    continue;
                }
                let depth = d.dot(AXIS);
                let far_face = map.lit(d, (1.0 + thickness) / depth, 1.0, 0.0);
                let before_wall = map.lit(d, 2.9 / depth, 1.0, 0.0);
                assert!(
                    (far_face - before_wall).abs() <= 1e-5 * full,
                    "{d}: {far_face} on the plate's far face, {before_wall} before the wall"
                );
                across += 1;
                if d.x / depth >= 1.5 * texel {
                    let inside = map.lit(d, 1.005 / depth, 1.0, 0.0);
                    assert_eq!(inside, 0.0, "{d}: 5 mm past the plate's near face");
                    within += 1;
                }
            }
        }
        assert!(across > 250 && within > 150, "{across} {within}");
    }

    /// Where every record direction about a point ended alike, the record decides, never the
    /// guide: a rod 2 cm across that no record direction meets, lying between directions that
    /// all ended on the wall, leaves the map reading the wall behind it, as the record does. The
    /// record is the outcome; a body the sim's directions missed is no shadow in it. Fixture: the
    /// rod of [`fixture_bodies`], read along its middle.
    #[test]
    fn agreeing_directions_are_never_overridden() {
        let bodies = fixture_bodies();
        let cone = cap(|_, d| guide(&bodies, d));
        let hits_rod = |d: Vec3| bodies.entry(Vec3::ZERO, d, GUIDE_REACH_M).unwrap().1 == 2;
        assert!(cone.rays.iter().all(|ray| !hits_rod(ray.direction)));
        let map = Map::of(&cone, Some(&bodies));
        let mut checked = 0;
        for k in 0..9 {
            let d = Vec3::new(-0.3 - 0.0125 * k as f32 - 0.0025, 0.0, -2.05).normalize();
            assert!(hits_rod(d));
            let got = map.sample(d);
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
        let through = cap(|_, _| 1.0)
            .rays
            .iter()
            .position(|ray| ray.direction.x > 0.05)
            .expect("a direction under the plate");
        let cone = cap(|k, d| {
            if k == through {
                3.0 / d.dot(AXIS)
            } else {
                guide(&bodies, d)
            }
        });
        let ray = &cone.rays[through];
        let map = Map::of(&cone, Some(&bodies));
        let samples = Samples::of(&map, Vec3::ZERO, &cone.rays, Some(&bodies));
        assert_eq!(samples.endings[through], Ending::Past { body: 1 });
        let got = samples.at(ray.direction, &mut 0);
        assert!(
            (got - ray.reach).abs() < 1e-4,
            "{got} against {}",
            ray.reach
        );
    }

    /// A field with no directions bakes to zero: nothing is drawn of it.
    #[test]
    fn a_field_with_no_directions_bakes_to_zero() {
        let field = FieldDescriptor::ball(2.5);
        let cone = Cone::new(Vec3::ZERO, AXIS, field, 0, 10.0, AT_ORIGIN, Vec::new());
        let map = Map::of(&cone, Some(&fixture_bodies()));
        assert!(map.texels.iter().all(|&t| t == 0.0));
    }

    /// The atlas holds each field's map in its own tile, texel for texel the reach [`Map::lit`]
    /// reads, and nothing else: the look's shader reads field `k` at [`tile`]`(k)`. A tile placed
    /// apart from where the shader reads it draws one field's shadows in another, and a reach
    /// rounded on its way to the texture moves the shader's shadows off the CPU read's.
    #[test]
    fn the_atlas_holds_each_map_in_its_tile() {
        let cones = [cap(|k, _| 1.0 + 0.01 * k as f32), band(|_, _| 2.0)];
        let maps = bake(&cones, None);
        let image = atlas(&maps);
        assert_eq!(image.texture_descriptor.format, TextureFormat::R32Float);
        assert_eq!(image.width() as usize, 2 * MAP_TEXELS);
        for (k, map) in maps.iter().enumerate() {
            for (i, j) in [(0, 0), (130, 140), (MAP_TEXELS - 1, MAP_TEXELS - 1)] {
                let at = tile(k) + UVec2::new(i as u32, j as u32);
                let got = image.get_color_at(at.x, at.y).unwrap().to_linear().red;
                assert_eq!(got, map.texels[j * MAP_TEXELS + i], "{k} {i} {j}");
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
