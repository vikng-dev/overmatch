//! A field's depth picture: from the field's origin, the reach to the struck tank's hard bodies
//! along every direction of its support, rendered on the GPU once per record and screen size. A
//! field is a spall cone or a burst's; its support is a cap about its axis out to its half-angle,
//! a ring's band from its inner angle out to it, or, for a ball whose rim is π, the whole sphere.
//!
//! The casters ([`Casters`]) are the struck tank's hard bodies ([`HardBodies`], every non-witness
//! ballistic primitive, posed as the record has the tank) and the horizon: the world-axis box
//! about them, where a direction that meets no body ends. A crew station is a witness and never a
//! caster (ADR-0040). A texel holds the radial distance (record m) from the origin to the nearest
//! caster face its centre direction meets head-on ([`CASTER_CULL`]), as a line enters a hard
//! body ([`HardBodies`]), so a line from inside a body leaves it.
//!
//! The picture is the cube about the field's axis (frame `u`, `v`, the axis; [`FRAMES`]), each face
//! cropped to the support ([`Crop`]) at the field's pitch (rad a texel, [`pitch_of`]), derived
//! from the live size of the target a view draws the record into ([`PictureDetail`]). A record's
//! faces share one atlas ([`Pictures`]), one `R32Float` texture rendered by
//! `pen_cam_picture.wgsl` ([`Pictures::image`], [`PictureJobs`]); every view of the record shares
//! it ([`RecordAtlas`], `game::RecordPictures`), laid out at the finest pitch any of them needs and
//! laid out anew, once a frame at most, when what they ask changes. A view draws the newest
//! pictures whose atlas's job has been encoded and submitted ([`PictureJobs::rendered`]): before
//! then an atlas holds nothing, so a record's first pictures keep its fields off, and pictures laid
//! out anew are drawn from the atlas before them until theirs is written. The look reads it
//! unfiltered (`pen_cam_look.wgsl`, `picture_at`): a point is tested against each of the four
//! texels about its direction, each against its own reach within a tolerance
//! ([`REACH_TOLERANCE_DIAGONALS`]), and the results weighed bilinearly; [`Picture::lit`] is the
//! same read on the CPU.
//!
//! The record wins: a field whose recorded directions all end at its origin holds no picture and
//! is drawn nowhere ([`Picture::of`]). Design doc §5, "Potency colour"
//! (`.agents/docs/design/armor-penetration-and-damage.md`).

use std::f32::consts::{PI, SQRT_2};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use avian3d::parry::query::RayCullingMode;
use bevy::asset::{RenderAssetUsages, StrongHandle};
use bevy::mesh::VertexBufferLayout;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::uniform_buffer;
use bevy::render::render_resource::encase::UniformBuffer;
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
    BufferInitDescriptor, BufferUsages, CachedRenderPipelineId, ColorTargetState, ColorWrites,
    CommandEncoderDescriptor, CompareFunction, DepthStencilState, Extent3d, Face, FragmentState,
    FrontFace, IndexFormat, LoadOp, MultisampleState, Operations, PipelineCache, PrimitiveState,
    PrimitiveTopology, RenderPassColorAttachment, RenderPassDepthStencilAttachment,
    RenderPassDescriptor, RenderPipeline, RenderPipelineDescriptor, ShaderStages, ShaderType,
    StoreOp, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
    TextureViewDescriptor, VertexFormat, VertexState, VertexStepMode,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::GpuImage;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};

use super::plan::Cone;
use crate::damage_cam::shapes::HardBodies;

/// The most fields one record's atlas holds.
pub const PICTURE_FIELDS: usize = 4;
/// A field's face lanes in the look's uniform: two for each face of the cube (`look::pack`).
pub const FACE_LANES: usize = 12;
/// The near plane (m along a face's forward): a caster face nearer the origin is not drawn.
/// `pen_cam_picture.wgsl` declares it.
pub const NEAR_M: f32 = 1.0e-4;
/// The caster faces the picture keeps: those a line from the origin meets head-on, every caster
/// wound outward (the manifold gate's certificate) and the horizon inward, as a line enters a hard
/// body ([`HardBodies`]).
pub const CASTER_CULL: Option<Face> = Some(Face::Back);
/// How far past a texel's own reach the look counts a point within it, at most: this many texel
/// diagonals of turn (`√2` pitches: a texel turns a direction by at most its pitch, the turn per
/// tangent unit of a cube face being at most 1, DERIVED) times the point's distance from the
/// origin, and never past the farthest reach of the four texels about the point's direction
/// ([`Picture::lit`]): the picture's slope-scaled bias. TUNED.
///
/// A hard body stops the field on its surface, so each texel's reach lands on the surface along
/// its own direction, and a surface point between texel directions lies past the reach of those
/// whose directions meet the surface nearer: read without a tolerance, the last stretch before the
/// surface flips texel by texel, a stair-step that stretches into streaks where the surface turns
/// edge-on to the field. With it a surface point reads inside wherever the reach grows by less
/// than the tolerance across the four texels: a surface up to `tan⁻¹ 2` = 63° off square to the
/// field at a face's centre, where the turn is steepest (DERIVED), and steeper toward its edges.
/// Where the four texels all end on a blocker, none reads past the farthest of them, so a point
/// behind it stays outside however near; across a silhouette a blocked texel reads past its blocker
/// only for a surface within the tolerance behind it.
pub const REACH_TOLERANCE_DIAGONALS: f32 = 2.0;
/// A face of the cube about a field's axis, `[right, up, forward]` in the field's local frame
/// (`u`, `v`, the axis), `right × up = −forward`: a direction is `forward + a right + b up` at
/// tangent `(a, b)` in `[−1, 1]²` on its face ([`face_of`]). `pen_cam_look.wgsl` declares the
/// same (`face_of`).
pub const FRAMES: [[Vec3; 3]; 6] = [
    [Vec3::X, Vec3::NEG_Y, Vec3::Z],
    [Vec3::X, Vec3::Y, Vec3::NEG_Z],
    [Vec3::Z, Vec3::Y, Vec3::X],
    [Vec3::NEG_Z, Vec3::Y, Vec3::NEG_X],
    [Vec3::X, Vec3::Z, Vec3::Y],
    [Vec3::X, Vec3::NEG_Z, Vec3::NEG_Y],
];
/// The cells a face's tangent square is cut into to find the support on it ([`Crop`]).
const CROP_GRID: usize = 128;
/// The step (physical px) a view's target size is rounded up to before a pitch is derived at it
/// ([`detail_of`]). The rounded size is never less than the live one, so a texel spans at most a
/// pixel of the live target at the sightings ([`pitch_of`]). A target that changes within its step
/// lays nothing anew; one that crosses a step lays its records' pictures anew, once. At a fixed
/// aspect the pitch goes as one over the target's height `h`, so the atlas holds at most
/// `((h + step) / h)²` of the texels `h` itself needs (DERIVED: 12 % more at 1080 px, 6 % at
/// 2160 px). TUNED.
pub const DETAIL_STEP_PX: u32 = 64;
/// The points along each recorded direction, out to its reach, a field's pitch is taken at.
const RAY_SAMPLES: usize = 16;
/// The pitch (rad) of a field no sighting sees. TUNED.
const COARSEST_PITCH: f32 = 1.0 / 32.0;
/// The atlas's side at most (texels): WebGPU's default `max_texture_dimension_2d`, which every
/// adapter offers.
const MAX_SIDE: u32 = 8192;
/// The factor every field's pitch grows by while a record's faces do not fit one atlas.
const PITCH_GROWTH: f32 = 1.25;
/// The atlas widths tried between the widest face and every face in one row.
const WIDTH_STEPS: u32 = 32;

/// The detail a view drawing into a target of `size` (physical px) asks of its records'
/// pictures: the size rounded up to [`DETAIL_STEP_PX`] on each side, a step at least.
pub fn detail_of(size: UVec2) -> UVec2 {
    let step = |px: u32| px.max(1).div_ceil(DETAIL_STEP_PX) * DETAIL_STEP_PX;
    UVec2::new(step(size.x), step(size.y))
}

/// The detail each view asks of the pictures this frame ([`detail_of`]), from the camera that
/// draws it (`hud::detail`): `corner`, the expanded corner's image, for a
/// [`Surface::Corner`](super::Surface) record at whatever size the corner stands, so expanding it
/// lays nothing anew; `world`, the 3D pass of the camera a [`Surface::World`](super::Surface)
/// record is drawn on; `xray`, the x-ray's camera's. `None` until its camera is known, and a view
/// asks nothing then and draws no field.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct PictureDetail {
    pub corner: Option<UVec2>,
    pub world: Option<UVec2>,
    pub xray: Option<UVec2>,
}

/// The frame about unit `axis` a field's picture is drawn in: `u` and `v` across it
/// (`u × v = axis`) and the axis.
pub fn frame(axis: Vec3) -> [Vec3; 3] {
    let (u, _) = axis.any_orthonormal_pair();
    [u, axis.cross(u), axis]
}

/// The face of the cube about a field's axis unit `local` (across `u` and `v`, along the axis)
/// lies on, its index in [`FRAMES`], and its tangent there. `pen_cam_look.wgsl`, `face_of`.
pub fn face_of(local: Vec3) -> (usize, Vec2) {
    let a = local.abs();
    if a.z >= a.x && a.z >= a.y {
        if local.z >= 0.0 {
            (0, Vec2::new(local.x, -local.y) / a.z)
        } else {
            (1, Vec2::new(local.x, local.y) / a.z)
        }
    } else if a.x >= a.y {
        if local.x >= 0.0 {
            (2, Vec2::new(local.z, local.y) / a.x)
        } else {
            (3, Vec2::new(-local.z, local.y) / a.x)
        }
    } else if local.y >= 0.0 {
        (4, Vec2::new(local.x, local.z) / a.y)
    } else {
        (5, Vec2::new(local.x, -local.z) / a.y)
    }
}

/// One face's crop: its tangent rectangle from `x0` across and down from `y1`, `size` texels of
/// the field's pitch, placed at `place` in the record's atlas. Texel `(i, j)`'s centre lies at
/// tangent `(x0 + (i + ½) pitch, y1 − (j + ½) pitch)`. A face the support misses is empty (`size`
/// zero), and the look reads nothing on it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Crop {
    pub x0: f32,
    pub y1: f32,
    pub size: UVec2,
    pub place: UVec2,
}

/// The cells of each face's [`CROP_GRID`]² that hold the support or lie within a cell diagonal
/// of it, as the low and high cell; `None` where none does.
type Cells = [Option<(UVec2, UVec2)>; 6];

/// The support's cells from its inner edge to its half-angle (rad off the axis): a cell is on
/// where its centre's direction lies within `√2` cells of the support (a cell turns a direction by
/// at most its width, DERIVED as [`REACH_TOLERANCE_DIAGONALS`]' turn), every cell for the whole
/// sphere.
fn support_cells(inner: f32, half_angle: f32) -> Cells {
    let cell = 2.0 / CROP_GRID as f32;
    let margin = SQRT_2 * cell;
    std::array::from_fn(|face| {
        let [right, up, forward] = FRAMES[face];
        let mut cells: Option<(UVec2, UVec2)> = None;
        for j in 0..CROP_GRID as u32 {
            for i in 0..CROP_GRID as u32 {
                let t = (UVec2::new(i, j).as_vec2() + 0.5) * cell - 1.0;
                let local = (forward + right * t.x + up * t.y).normalize();
                let theta = local.z.clamp(-1.0, 1.0).acos();
                let on =
                    half_angle >= PI || (theta >= inner - margin && theta <= half_angle + margin);
                if on {
                    let at = UVec2::new(i, j);
                    cells = Some(cells.map_or((at, at), |(lo, hi)| (lo.min(at), hi.max(at))));
                }
            }
        }
        cells
    })
}

impl Crop {
    /// The crop over `cells` (low and high) at `pitch` (rad a texel): their rectangle and one
    /// texel past it each side, so the four texels about any direction on it lie inside, at least
    /// two texels a side.
    fn of(cells: Option<(UVec2, UVec2)>, pitch: f32) -> Self {
        let Some((lo, hi)) = cells else {
            return Self::default();
        };
        let cell = 2.0 / CROP_GRID as f32;
        let low = lo.as_vec2() * cell - 1.0 - pitch;
        let high = (hi + 1).as_vec2() * cell - 1.0 + pitch;
        let size = ((high - low) / pitch)
            .ceil()
            .max(Vec2::splat(2.0))
            .as_uvec2();
        Self {
            x0: low.x,
            y1: high.y,
            size,
            place: UVec2::ZERO,
        }
    }

    /// Its tangent rectangle `(x0, y0, x1, y1)` at `pitch`.
    fn rect(self, pitch: f32) -> Vec4 {
        let span = self.size.as_vec2() * pitch;
        Vec4::new(self.x0, self.y1 - span.y, self.x0 + span.x, self.y1)
    }
}

/// One field's picture: its origin and frame (record frame; `u`, `v` across the unit axis,
/// `u × v = axis`), its support's half-angle and inner edge (rad), its pitch (rad a texel), each
/// face's crop, and the longest reach the record gives it (m), past which the look draws none of
/// it.
#[derive(Clone, Debug)]
pub struct Picture {
    pub origin: Vec3,
    pub axis: Vec3,
    pub u: Vec3,
    pub v: Vec3,
    pub half_angle: f32,
    pub inner: f32,
    pub pitch: f32,
    pub crops: [Crop; 6],
    pub longest: f32,
}

impl Picture {
    /// `cone`'s picture at `pitch`, unplaced. A field with no recorded direction off its origin
    /// (every fragment ended where it was thrown) has every face empty: the record wins, and the
    /// look draws nothing of it.
    pub fn of(cone: &Cone, pitch: f32) -> Self {
        Self::with_cells(cone, &Self::cells(cone), pitch)
    }

    /// The cells `cone`'s support covers on each face; none where the record wins.
    fn cells(cone: &Cone) -> Cells {
        if cone.rays.is_empty() {
            [None; 6]
        } else {
            support_cells(cone.inner(), cone.half_angle())
        }
    }

    fn with_cells(cone: &Cone, cells: &Cells, pitch: f32) -> Self {
        let [u, v, axis] = frame(cone.axis);
        Self {
            origin: cone.origin,
            axis,
            u,
            v,
            half_angle: cone.half_angle(),
            inner: cone.inner(),
            pitch,
            crops: std::array::from_fn(|face| Crop::of(cells[face], pitch)),
            longest: cone.max_reach(),
        }
    }

    /// Unit `direction` (record frame) in the field's local frame: across `u` and `v`, along the
    /// axis.
    pub fn local(&self, direction: Vec3) -> Vec3 {
        Vec3::new(
            direction.dot(self.u),
            direction.dot(self.v),
            direction.dot(self.axis),
        )
    }

    /// `local` in the record frame.
    fn record(&self, local: Vec3) -> Vec3 {
        self.u * local.x + self.v * local.y + self.axis * local.z
    }

    /// The face lanes the look reads its crops from (`look::pack`): face `k`'s tangent corner, its
    /// size (texels) and its place in the atlas.
    pub fn lanes(&self) -> [Vec4; FACE_LANES] {
        std::array::from_fn(|lane| {
            let crop = self.crops[lane / 2];
            if lane % 2 == 0 {
                Vec4::new(crop.x0, crop.y1, crop.size.x as f32, crop.size.y as f32)
            } else {
                Vec4::new(crop.place.x as f32, crop.place.y as f32, 0.0, 0.0)
            }
        })
    }

    /// The unit direction (record frame) at the centre of texel `texel` of face `face`.
    fn texel(&self, face: usize, texel: UVec2) -> Vec3 {
        let crop = self.crops[face];
        let at = texel.as_vec2() + 0.5;
        let t = Vec2::new(crop.x0 + at.x * self.pitch, crop.y1 - at.y * self.pitch);
        let [right, up, forward] = FRAMES[face];
        self.record((forward + right * t.x + up * t.y).normalize())
    }

    /// The share (0..1) of the field lit along the direction `local` ([`Self::local`]) at `r`
    /// world m from its origin, `scale` world m per record m, `reach` the reach (record m) a texel
    /// holds along its centre direction (record frame): each of the four texels about the
    /// direction lights where `r` lies within its own reach, out to its tolerance past it
    /// ([`REACH_TOLERANCE_DIAGONALS`], never past the farthest of the four) and `slack` world m
    /// past that, and the four are weighted bilinearly. A reach is compared per texel and never
    /// interpolated, so across a silhouette a point behind the blocker, past the tolerance, gets
    /// the open texels' share alone, the same at every depth out to the wall. An empty face lights
    /// nothing. The four texels are always the direction's own face's (its crop runs a texel past
    /// the face's edge) and no face is blended with its neighbour: a silhouette across a face's
    /// edge is drawn within half a texel of it on either side, so it jogs there by at most a
    /// texel, under a pixel of the live target at the sightings ([`pitch_of`]; design doc §5,
    /// "Potency colour"). `pen_cam_look.wgsl`'s `picture_at` reads the atlas so; this is the read
    /// it mirrors, held by the tests.
    pub fn lit(
        &self,
        local: Vec3,
        r: f32,
        scale: f32,
        slack: f32,
        reach: &dyn Fn(Vec3) -> f32,
    ) -> f32 {
        let (face, t) = face_of(local);
        let crop = self.crops[face];
        if crop.size.x < 2 || crop.size.y < 2 {
            return 0.0;
        }
        let g = Vec2::new((t.x - crop.x0) / self.pitch, (crop.y1 - t.y) / self.pitch) - 0.5;
        let low = g.floor().clamp(Vec2::ZERO, (crop.size - 2).as_vec2());
        let f = (g - low).clamp(Vec2::ZERO, Vec2::ONE);
        let at = low.as_uvec2();
        let texel = |i: u32, j: u32| reach(self.texel(face, at + UVec2::new(i, j))) * scale;
        let reaches = [texel(0, 0), texel(1, 0), texel(0, 1), texel(1, 1)];
        let farthest = reaches.into_iter().fold(0.0, f32::max);
        let tolerance = REACH_TOLERANCE_DIAGONALS * SQRT_2 * self.pitch * r;
        let [a, b, c, d] = reaches.map(|reach| {
            let met = farthest.min(reach + tolerance) + slack;
            if r <= met { 1.0 } else { 0.0 }
        });
        let (below, above) = (a + (b - a) * f.x, c + (d - c) * f.x);
        below + (above - below) * f.y
    }

    /// Its texels, every face's crop together.
    pub fn texels(&self) -> u64 {
        self.crops
            .iter()
            .map(|crop| u64::from(crop.size.x) * u64::from(crop.size.y))
            .sum()
    }
}

/// A field's picture and the reach its texels hold, as the CPU mirrors read it: `reach` along a
/// unit direction (record frame) is what a texel whose centre lies along it holds.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the look's shader reads the pictures at runtime; the CPU mirrors read them so, held by the tests"
    )
)]
#[derive(Clone, Copy)]
pub struct Seen<'a> {
    pub picture: &'a Picture,
    pub reach: &'a dyn Fn(Vec3) -> f32,
}

/// What a record's pictures are rendered from: the struck tank's hard bodies, and the horizon,
/// the world-axis box about them (record frame, low and high corners), where a direction that
/// meets no body ends.
#[derive(Debug)]
pub struct Casters {
    bodies: Arc<HardBodies>,
    lo: Vec3,
    hi: Vec3,
}

impl Casters {
    /// The casters of `bodies`; `None` for bodies with no vertex.
    pub fn of(bodies: &Arc<HardBodies>) -> Option<Self> {
        let (lo, hi) = bodies.vertices().iter().fold(None, |bounds, &p| {
            Some(bounds.map_or((p, p), |(lo, hi): (Vec3, Vec3)| (lo.min(p), hi.max(p))))
        })?;
        Some(Self {
            bodies: bodies.clone(),
            lo,
            hi,
        })
    }

    /// The mesh the picture is rendered from: the bodies' vertices and triangles, then the
    /// horizon's eight corners and twelve triangles, wound inward, so a line from inside the box
    /// meets each of its faces head-on.
    fn mesh(&self) -> (Vec<Vec3>, Vec<u32>) {
        let mut vertices = self.bodies.vertices().to_vec();
        let mut indices: Vec<u32> = self.bodies.triangles().iter().flatten().copied().collect();
        let base = vertices.len() as u32;
        let (lo, hi) = (self.lo, self.hi);
        vertices.extend((0..8).map(|k| {
            Vec3::new(
                if k & 1 == 0 { lo.x } else { hi.x },
                if k & 2 == 0 { lo.y } else { hi.y },
                if k & 4 == 0 { lo.z } else { hi.z },
            )
        }));
        let centre = 0.5 * (lo + hi);
        let corner = |k: u32| vertices[(base + k) as usize];
        for [a, b, c, d] in [
            [0, 1, 3, 2],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 3, 7, 5],
        ] {
            for [x, y, z] in [[a, b, c], [a, c, d]] {
                let normal = (corner(y) - corner(x)).cross(corner(z) - corner(x));
                let inward = normal.dot(centre - corner(x)) > 0.0;
                let t = if inward { [x, y, z] } else { [x, z, y] };
                indices.extend(t.map(|k| base + k));
            }
        }
        (vertices, indices)
    }

    /// Where along the line from `origin` along unit `direction` (record frame) the picture of a
    /// field about unit `axis` ends (m): the nearest caster face it meets head-on
    /// ([`CASTER_CULL`]) past the near plane, or the horizon's far side; 0 where the line misses
    /// the horizon. The near plane lies [`NEAR_M`] along the forward of the face of the cube about
    /// `axis` ([`frame`]) the direction lies on ([`face_of`]), so `NEAR_M` over its cosine along
    /// the direction, as the GPU clips each face (`pen_cam_picture.wgsl`; a crop's one-texel
    /// overhang past its face's edge is clipped along its own face's forward, which moves the near
    /// plane there by at most `NEAR_M √3` pitch, DERIVED). The pictures' CPU mirror, held by the
    /// tests against the record; no test reads the GPU's pictures back.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the GPU renders the pictures at runtime; this is the reach it mirrors, held by the tests"
        )
    )]
    pub fn reach(&self, origin: Vec3, axis: Vec3, direction: Vec3) -> f32 {
        let [u, v, axis] = frame(axis);
        let local = Vec3::new(direction.dot(u), direction.dot(v), direction.dot(axis));
        let near = NEAR_M / local.abs().max_element();
        let start = origin + direction * near;
        let inverse = direction.recip();
        let (a, b) = ((self.lo - start) * inverse, (self.hi - start) * inverse);
        let (enter, leave) = (a.min(b).max_element(), a.max(b).min_element());
        if !leave.is_finite() || leave < enter.max(0.0) {
            return 0.0;
        }
        let culling = CASTER_CULL.map(|face| match face {
            Face::Back => RayCullingMode::IgnoreBackfaces,
            Face::Front => RayCullingMode::IgnoreFrontfaces,
        });
        let hit = self
            .bodies
            .cast(start, direction, leave, culling)
            .unwrap_or(leave);
        hit.min(leave) + near
    }
}

/// Field `cone`'s pitch (rad a texel) for a view drawing into a target of `detail` (physical px,
/// [`detail_of`]): over every `sightings` (a corner camera's pose and lens in the record frame, its
/// lens fitted to `detail`), at every point within view along every recorded direction
/// ([`RAY_SAMPLES`] out to its reach), the angle one pixel spans there seen from the field's
/// origin, the least of them; [`COARSEST_PITCH`] at most. A texel spans at most a pixel at those
/// points alone: the sightings and the points are samples of what the views draw, a point between
/// them may take a finer pixel, and a record whose faces do not fit one atlas has every pitch grown
/// ([`PITCH_GROWTH`]).
pub fn pitch_of(cone: &Cone, sightings: &[(Transform, Projection)], detail: UVec2) -> f32 {
    let size = detail.max(UVec2::ONE).as_vec2();
    let mut pitch = COARSEST_PITCH;
    for (eye, lens) in sightings {
        let mut lens = lens.clone();
        lens.update(size.x, size.y);
        let clip_from_view = lens.get_clip_from_view();
        let clip_from_world = clip_from_view * eye.to_matrix().inverse();
        for ray in &cone.rays {
            for s in 1..=RAY_SAMPLES {
                let r = ray.reach * s as f32 / RAY_SAMPLES as f32;
                let clip = clip_from_world * (cone.origin + ray.direction * r).extend(1.0);
                let seen = clip.w > 0.0
                    && clip.x.abs() <= clip.w
                    && clip.y.abs() <= clip.w
                    && (0.0..=clip.w).contains(&clip.z);
                if seen && r > 0.0 {
                    let pixel = 2.0 * clip.w / (clip_from_view.y_axis.y * size.y);
                    pitch = pitch.min(pixel / r);
                }
            }
        }
    }
    pitch
}

/// Places `sizes` (texels) in one atlas no side of which passes [`MAX_SIDE`]: the skyline's lowest
/// spot for each, tallest first, over [`WIDTH_STEPS`] widths from the widest's to all in a row,
/// the least area kept. Each place and the atlas's size; `None` where they do not fit. An empty
/// size takes no room.
fn pack(sizes: &[UVec2]) -> Option<(Vec<UVec2>, UVec2)> {
    let mut order: Vec<usize> = (0..sizes.len())
        .filter(|&k| sizes[k].x > 0 && sizes[k].y > 0)
        .collect();
    if order.is_empty() {
        return Some((vec![UVec2::ZERO; sizes.len()], UVec2::ZERO));
    }
    order.sort_by_key(|&k| std::cmp::Reverse((sizes[k].y, sizes[k].x)));
    let widest = order.iter().map(|&k| sizes[k].x).max()?;
    let row = order.iter().map(|&k| sizes[k].x).sum::<u32>().min(MAX_SIDE);
    if widest > MAX_SIDE {
        return None;
    }
    let mut best: Option<(u64, Vec<UVec2>, UVec2)> = None;
    for step in 0..=WIDTH_STEPS {
        let grow = (row as f32 / widest as f32).powf(step as f32 / WIDTH_STEPS as f32);
        let width = ((widest as f32 * grow).round() as u32).clamp(widest, row);
        let (places, size) = skyline(sizes, &order, width);
        if size.y > MAX_SIDE {
            continue;
        }
        let area = u64::from(size.x) * u64::from(size.y);
        if best.as_ref().is_none_or(|(least, ..)| area < *least) {
            best = Some((area, places, size));
        }
    }
    best.map(|(_, places, size)| (places, size))
}

/// `sizes` in `order` on a skyline `width` texels wide: each at the lowest top its width finds,
/// the leftmost of equals. Each place and the extent used.
fn skyline(sizes: &[UVec2], order: &[usize], width: u32) -> (Vec<UVec2>, UVec2) {
    let mut places = vec![UVec2::ZERO; sizes.len()];
    // Segments `(x, top)`, each running to the next one's `x` and the last to `width`.
    let mut line: Vec<(u32, u32)> = vec![(0, 0)];
    let mut extent = UVec2::ZERO;
    for &k in order {
        let size = sizes[k];
        let mut spot: Option<(u32, u32)> = None;
        for s in 0..line.len() {
            let x = line[s].0;
            if x + size.x > width {
                break;
            }
            let top = line[s..]
                .iter()
                .take_while(|&&(start, _)| start < x + size.x)
                .map(|&(_, top)| top)
                .max()
                .unwrap_or(0);
            if spot.is_none_or(|(least, _)| top < least) {
                spot = Some((top, x));
            }
        }
        let (top, x) = spot.unwrap_or((extent.y, 0));
        places[k] = UVec2::new(x, top);
        extent = extent.max(UVec2::new(x + size.x, top + size.y));
        let end = x + size.x;
        let tail = line.iter().rev().find(|&&(start, _)| start <= end).copied();
        line.retain(|&(start, _)| start < x || start >= end);
        let at = line.partition_point(|&(start, _)| start < x);
        line.insert(at, (x, top + size.y));
        if end < width
            && let Some((start, height)) = tail
            && start < end
        {
            line.insert(at + 1, (end, height));
        }
        line.dedup_by(|later, earlier| later.1 == earlier.1);
    }
    (places, extent)
}

/// A record's pictures: the first [`PICTURE_FIELDS`] of its fields, each face placed in one
/// atlas of `size` texels, rendered from `casters` the first time a frame asks
/// ([`Self::image`]).
#[derive(Debug)]
pub struct Pictures {
    pub fields: Vec<Picture>,
    pub size: UVec2,
    /// The details the pitches were derived at, ascending ([`Self::of`]).
    pub details: Vec<UVec2>,
    casters: Option<Casters>,
    /// The atlas while any handle to it lives: every frame of the record reads the one image.
    image: Mutex<Weak<StrongHandle>>,
}

impl Pictures {
    /// The pictures of `cones` from the struck tank's `bodies` (record frame; `None` without the
    /// tank asset's shapes, and nothing is drawn), each at the finest pitch `sightings` derive at
    /// any of `details` ([`pitch_of`]; [`COARSEST_PITCH`] at none), every pitch grown by
    /// [`PITCH_GROWTH`] until the faces fit one atlas.
    pub fn of(
        cones: &[Cone],
        bodies: Option<&Arc<HardBodies>>,
        sightings: &[(Transform, Projection)],
        details: &[UVec2],
    ) -> Self {
        let cones = &cones[..cones.len().min(PICTURE_FIELDS)];
        let cells: Vec<Cells> = cones.iter().map(Picture::cells).collect();
        let mut pitches: Vec<f32> = cones
            .iter()
            .map(|cone| {
                details
                    .iter()
                    .map(|&detail| pitch_of(cone, sightings, detail))
                    .fold(COARSEST_PITCH, f32::min)
            })
            .collect();
        let mut details = details.to_vec();
        details.sort_by_key(|detail| (detail.x, detail.y));
        details.dedup();
        loop {
            let mut fields: Vec<Picture> = cones
                .iter()
                .zip(&cells)
                .zip(&pitches)
                .map(|((cone, cells), &pitch)| Picture::with_cells(cone, cells, pitch))
                .collect();
            let sizes: Vec<UVec2> = fields
                .iter()
                .flat_map(|field| field.crops.map(|crop| crop.size))
                .collect();
            if let Some((places, size)) = pack(&sizes) {
                for (crop, place) in fields
                    .iter_mut()
                    .flat_map(|field| field.crops.iter_mut())
                    .zip(places)
                {
                    crop.place = place;
                }
                return Self {
                    fields,
                    size,
                    details,
                    casters: bodies.and_then(Casters::of),
                    image: Mutex::default(),
                };
            }
            for pitch in &mut pitches {
                *pitch *= PITCH_GROWTH;
            }
        }
    }

    /// The atlas: the one image while any handle to it lives, made anew and queued for rendering
    /// otherwise, its texels written once its job is submitted ([`PictureJobs::rendered`]).
    /// `None` without casters or without a face to draw.
    pub fn image(
        self: &Arc<Self>,
        images: &mut Assets<Image>,
        jobs: &mut PictureJobs,
    ) -> Option<Handle<Image>> {
        if self.casters.is_none() || self.size.x == 0 || self.size.y == 0 {
            return None;
        }
        let mut held = self.image.lock().ok()?;
        if let Some(strong) = held.upgrade() {
            return Some(Handle::Strong(strong));
        }
        let mut image = Image::new_uninit(
            Extent3d {
                width: self.size.x,
                height: self.size.y,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            TextureFormat::R32Float,
            RenderAssetUsages::default(),
        );
        image.texture_descriptor.usage |= TextureUsages::RENDER_ATTACHMENT;
        let handle = images.add(image);
        if let Handle::Strong(strong) = &handle {
            *held = Arc::downgrade(strong);
        }
        jobs.0.push(PictureJob {
            image: handle.clone(),
            pictures: self.clone(),
            done: Arc::default(),
        });
        Some(handle)
    }
}

/// How a record's pictures are laid out at the details its views ask ([`Pictures::of`] over the
/// record's fields, its struck tank's bodies and its sightings): the first showing plan's.
pub type Lay = Box<dyn Fn(&[UVec2]) -> Pictures + Send + Sync>;

/// A record's depth pictures as its views draw them, one for every plan of the record
/// (`Plan::adopt_pictures`): laid out at every detail the views showing the record ask
/// ([`Self::show`]), so each field takes the finest pitch any of them needs at its own live size,
/// and laid out anew, once, at the end of a frame whose asks differ from the newest pictures'
/// ([`PictureJobs::realize`]). A view draws the newest pictures whose atlas has been submitted
/// ([`PictureJobs::rendered`]): pictures laid out anew are drawn from the atlas before them until
/// theirs is written, so a change of size draws no frame without the fields.
#[derive(Default)]
pub struct RecordAtlas(Mutex<Laying>);

#[derive(Default)]
struct Laying {
    /// Each view's ask while it shows the record: its token ([`ViewPictures`]) and its detail.
    asks: Vec<(Weak<()>, UVec2)>,
    lay: Option<Lay>,
    /// The newest pictures whose atlas has been submitted.
    drawn: Option<Arc<Pictures>>,
    /// The pictures laid out at the details asked since, until their atlas has been submitted.
    next: Option<Arc<Pictures>>,
}

impl RecordAtlas {
    /// Show the record's pictures in `view`, which asks `detail` ([`detail_of`]): its ask renewed
    /// and the record left for the frame's end to lay out at every detail asked
    /// ([`PictureJobs::realize`]), by `lay` where no view has given one yet; the next pictures
    /// drawn once their atlas is submitted; `view` drawing the newest pictures whose atlas has
    /// been, and holding every atlas still to be written so its job is kept.
    pub fn show(
        self: &Arc<Self>,
        view: &mut ViewPictures,
        detail: UVec2,
        lay: impl FnOnce() -> Lay,
        images: &mut Assets<Image>,
        jobs: &mut PictureJobs,
    ) {
        let Ok(mut laying) = self.0.lock() else {
            return;
        };
        let token = Arc::downgrade(&view.token);
        laying
            .asks
            .retain(|(held, _)| held.strong_count() > 0 && !held.ptr_eq(&token));
        laying.asks.push((token, detail));
        laying.lay.get_or_insert_with(lay);
        if !jobs.1.iter().any(|asked| Arc::ptr_eq(asked, self)) {
            jobs.1.push(self.clone());
        }
        let next = laying.next.as_ref().map(|next| next.image(images, jobs));
        if let Some(atlas) = next
            && jobs.rendered(atlas.as_ref())
        {
            laying.drawn = laying.next.take();
        }
        // The view's atlases are taken anew while it still holds last frame's: an atlas let go
        // before it is taken again is allocated and rendered anew.
        let (mut drawn, mut held) = (None, Vec::new());
        if let Some(pictures) = &laying.drawn
            && let Some(atlas) = pictures.image(images, jobs)
        {
            if jobs.rendered(Some(&atlas)) {
                drawn = Some((pictures.clone(), atlas.clone()));
            }
            held.push(atlas);
        }
        if let Some(atlas) = laying
            .next
            .as_ref()
            .and_then(|next| next.image(images, jobs))
        {
            held.push(atlas);
        }
        (view.drawn, view.held) = (drawn, held);
    }

    /// Lay the pictures out at every detail a live view asks where the newest pictures' differ:
    /// the next pictures, their atlas made and queued ([`Pictures::image`]), its job holding it
    /// until the views take it the next frame. A next laid out at other details goes.
    fn realize(&self, images: &mut Assets<Image>, jobs: &mut PictureJobs) {
        let Ok(mut laying) = self.0.lock() else {
            return;
        };
        let mut details: Vec<UVec2> = laying
            .asks
            .iter()
            .filter(|(held, _)| held.strong_count() > 0)
            .map(|&(_, detail)| detail)
            .collect();
        details.sort_by_key(|detail| (detail.x, detail.y));
        details.dedup();
        let laid = |pictures: &Option<Arc<Pictures>>| {
            pictures
                .as_ref()
                .is_some_and(|pictures| pictures.details == details)
        };
        if laid(&laying.next) {
            return;
        }
        laying.next = None;
        if laid(&laying.drawn) {
            return;
        }
        let Some(pictures) = laying.lay.as_ref().map(|lay| Arc::new(lay(&details))) else {
            return;
        };
        debug!(
            "pen cam: a record's pictures laid out at {details:?}: atlas {}x{}",
            pictures.size.x, pictures.size.y
        );
        pictures.image(images, jobs);
        laying.next = Some(pictures);
    }
}

/// One view's hold on the record it shows ([`RecordAtlas::show`]): replaced when the view shows
/// another record, which withdraws its ask from the one before.
#[derive(Default)]
pub struct ViewPictures {
    /// The view's ask in the record's [`RecordAtlas`], withdrawn when this goes.
    token: Arc<()>,
    /// The pictures the view draws and their atlas, once submitted.
    drawn: Option<(Arc<Pictures>, Handle<Image>)>,
    /// The atlases kept for the view while they are drawn or written: the drawn pictures' and the
    /// next's.
    held: Vec<Handle<Image>>,
}

impl ViewPictures {
    /// The pictures the view draws and the atlas they are read from; `None` before any is
    /// written.
    pub fn drawn(&self) -> Option<&(Arc<Pictures>, Handle<Image>)> {
        self.drawn.as_ref()
    }
}

/// The pictures waiting to be rendered, each into its atlas: a job waits until the render world
/// has encoded and submitted it ([`render_jobs`]), is submitted once, and leaves once submitted,
/// or once nothing but it holds its atlas ([`Self::retire`]). Beside them, the records a view
/// asked this frame, laid out at its end ([`Self::realize`]).
#[derive(Resource, Default)]
pub struct PictureJobs(Vec<PictureJob>, Vec<Arc<RecordAtlas>>);

impl PictureJobs {
    /// Whether `atlas` may be read: no job of it waits to be submitted. Until then the atlas holds
    /// nothing, and a view keeps the fields that read it off. No atlas waits on nothing.
    pub fn rendered(&self, atlas: Option<&Handle<Image>>) -> bool {
        atlas.is_none_or(|atlas| {
            !self
                .0
                .iter()
                .any(|job| job.image.id() == atlas.id() && !job.done.load(Ordering::Acquire))
        })
    }

    /// The jobs the render world takes this frame: those not yet submitted.
    fn waiting(&self) -> impl Iterator<Item = RenderPictureJob> + '_ {
        self.0
            .iter()
            .filter(|job| !job.done.load(Ordering::Acquire))
            .map(|job| RenderPictureJob {
                image: job.image.id(),
                pictures: job.pictures.clone(),
                done: job.done.clone(),
            })
    }

    /// Drop the jobs submitted, and those whose atlas nothing but the job holds: a record no view
    /// shows any more is never rendered.
    fn retire(&mut self) {
        self.0.retain(|job| {
            let held = matches!(&job.image, Handle::Strong(atlas) if Arc::strong_count(atlas) > 1);
            held && !job.done.load(Ordering::Acquire)
        });
    }

    /// Lay out each record a view asked this frame ([`RecordAtlas::show`]), once every view has
    /// asked: a record's views lay one layout out a frame between them, whatever order they ask
    /// in.
    fn realize(&mut self, images: &mut Assets<Image>) {
        for record in std::mem::take(&mut self.1) {
            record.realize(images, self);
        }
    }

    /// Stand in for the render world, which has no GPU under test: every waiting job submitted, as
    /// [`render_jobs`] marks one once [`encode`] has submitted it.
    #[cfg(test)]
    pub fn submit_all(&self) {
        for job in &self.0 {
            job.done.store(true, Ordering::Release);
        }
    }

    /// The jobs held, submitted or not.
    #[cfg(test)]
    pub fn queued(&self) -> usize {
        self.0.len()
    }
}

struct PictureJob {
    image: Handle<Image>,
    pictures: Arc<Pictures>,
    done: Arc<AtomicBool>,
}

/// The render world's copy of [`PictureJobs`], taken each frame.
#[derive(Resource, Default)]
struct RenderPictureJobs(Vec<RenderPictureJob>);

struct RenderPictureJob {
    image: AssetId<Image>,
    pictures: Arc<Pictures>,
    done: Arc<AtomicBool>,
}

/// The pictures' pipeline and its one bind group's layout: the face's uniform.
#[derive(Resource)]
struct PicturePipeline {
    layout: BindGroupLayoutDescriptor,
    id: CachedRenderPipelineId,
}

/// One face's uniform (`pen_cam_picture.wgsl`, `Face`): its frame (record frame), the field's
/// origin and the crop's tangent rectangle `(x0, y0, x1, y1)`.
#[derive(ShaderType, Clone, Copy, Debug)]
struct FaceUniform {
    right: Vec4,
    up: Vec4,
    forward: Vec4,
    origin: Vec4,
    rect: Vec4,
}

impl FaceUniform {
    fn of(picture: &Picture, face: usize) -> Self {
        let [right, up, forward] = FRAMES[face].map(|axis| picture.record(axis).extend(0.0));
        Self {
            right,
            up,
            forward,
            origin: picture.origin.extend(1.0),
            rect: picture.crops[face].rect(picture.pitch),
        }
    }
}

/// The casters' rasterization: triangles, counter-clockwise in the face's clip space where they
/// face the origin, the rest culled ([`CASTER_CULL`]).
fn caster_primitive() -> PrimitiveState {
    PrimitiveState {
        topology: PrimitiveTopology::TriangleList,
        front_face: FrontFace::Ccw,
        cull_mode: CASTER_CULL,
        ..default()
    }
}

/// The casters' depth: reverse (`NEAR / z`), the nearest kept.
fn caster_depth() -> DepthStencilState {
    DepthStencilState {
        format: TextureFormat::Depth32Float,
        depth_write_enabled: Some(true),
        depth_compare: Some(CompareFunction::Greater),
        stencil: default(),
        bias: default(),
    }
}

/// The reach's target: one full float a texel, written whole.
fn reach_target() -> ColorTargetState {
    ColorTargetState {
        format: TextureFormat::R32Float,
        blend: None,
        write_mask: ColorWrites::ALL,
    }
}

fn queue_pipeline(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    asset_server: Res<AssetServer>,
) {
    let shader = asset_server.load("shaders/pen_cam_picture.wgsl");
    let layout = BindGroupLayoutDescriptor::new(
        "pen_cam_picture",
        &BindGroupLayoutEntries::single(
            ShaderStages::VERTEX_FRAGMENT,
            uniform_buffer::<FaceUniform>(false),
        ),
    );
    let id = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("pen_cam_picture".into()),
        layout: vec![layout.clone()],
        immediate_size: 0,
        vertex: VertexState {
            shader: shader.clone(),
            shader_defs: Vec::new(),
            entry_point: Some("vertex".into()),
            buffers: vec![VertexBufferLayout::from_vertex_formats(
                VertexStepMode::Vertex,
                [VertexFormat::Float32x3],
            )],
        },
        primitive: caster_primitive(),
        depth_stencil: Some(caster_depth()),
        multisample: MultisampleState::default(),
        fragment: Some(FragmentState {
            shader,
            shader_defs: Vec::new(),
            entry_point: Some("fragment".into()),
            targets: vec![Some(reach_target())],
        }),
        zero_initialize_workgroup_memory: false,
    });
    commands.insert_resource(PicturePipeline { layout, id });
}

fn extract_jobs(mut render: ResMut<RenderPictureJobs>, jobs: Extract<Option<Res<PictureJobs>>>) {
    render.0.clear();
    let Some(jobs) = jobs.as_ref() else {
        return;
    };
    render.0.extend(jobs.waiting());
}

/// Render each waiting job's pictures into its atlas once the pipeline and the atlas's texture
/// are ready: one pass, before the frame's own.
fn render_jobs(
    jobs: Res<RenderPictureJobs>,
    pipeline: Option<Res<PicturePipeline>>,
    cache: Res<PipelineCache>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let Some(pipeline) = pipeline else {
        return;
    };
    let Some(render) = cache.get_render_pipeline(pipeline.id) else {
        return;
    };
    let layout = cache.get_bind_group_layout(&pipeline.layout);
    for job in &jobs.0 {
        if job.done.load(Ordering::Acquire) {
            continue;
        }
        let Some(target) = images.get(job.image) else {
            continue;
        };
        encode(
            &device,
            &queue,
            render,
            &layout,
            &job.pictures,
            &target.texture_view,
        );
        job.done.store(true, Ordering::Release);
    }
}

/// Draw `pictures`' casters into `target`, its atlas, once for each face with a crop, cleared to
/// zero first, and submit.
fn encode(
    device: &RenderDevice,
    queue: &RenderQueue,
    pipeline: &RenderPipeline,
    layout: &BindGroupLayout,
    pictures: &Pictures,
    target: &TextureView,
) {
    let Some(casters) = &pictures.casters else {
        return;
    };
    let (vertices, indices) = casters.mesh();
    let vertex_bytes: Vec<u8> = vertices
        .iter()
        .flat_map(|v| v.to_array())
        .flat_map(f32::to_le_bytes)
        .collect();
    let index_bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    let vertex = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("pen_cam_picture"),
        contents: &vertex_bytes,
        usage: BufferUsages::VERTEX,
    });
    let index = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("pen_cam_picture"),
        contents: &index_bytes,
        usage: BufferUsages::INDEX,
    });
    let depth = device.create_texture(&TextureDescriptor {
        label: Some("pen_cam_picture_depth"),
        size: Extent3d {
            width: pictures.size.x,
            height: pictures.size.y,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Depth32Float,
        usage: TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth = depth.create_view(&TextureViewDescriptor::default());
    let faces: Vec<_> = pictures
        .fields
        .iter()
        .flat_map(|picture| (0..6).map(move |face| (picture, face)))
        .filter(|(picture, face)| picture.crops[*face].size.min_element() >= 2)
        .filter_map(|(picture, face)| {
            let uniform =
                UniformBuffer::<Vec<u8>>::content_of::<_, Vec<u8>>(&FaceUniform::of(picture, face))
                    .ok()?;
            let buffer = device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("pen_cam_picture_face"),
                contents: &uniform,
                usage: BufferUsages::UNIFORM,
            });
            let group = device.create_bind_group(
                "pen_cam_picture_face",
                layout,
                &BindGroupEntries::single(buffer.as_entire_binding()),
            );
            Some((picture.crops[face], group))
        })
        .collect();
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("pen_cam_picture"),
    });
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("pen_cam_picture"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(Operations {
                    load: LoadOp::Clear(0.0),
                    store: StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..default()
        });
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(0, *vertex.slice(..));
        pass.set_index_buffer(*index.slice(..), IndexFormat::Uint32);
        for (crop, group) in &faces {
            let (place, size) = (crop.place, crop.size);
            pass.set_viewport(
                place.x as f32,
                place.y as f32,
                size.x as f32,
                size.y as f32,
                0.0,
                1.0,
            );
            pass.set_scissor_rect(place.x, place.y, size.x, size.y);
            pass.set_bind_group(0, &**group, &[]);
            pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
        }
    }
    queue.submit([encoder.finish()]);
}

fn retire(mut jobs: ResMut<PictureJobs>) {
    jobs.retire();
}

fn realize(mut jobs: ResMut<PictureJobs>, mut images: ResMut<Assets<Image>>) {
    jobs.realize(&mut images);
}

/// The render world's half: the pipeline, the jobs' extraction and their rendering, before the
/// frame's own passes. Nothing without a renderer.
pub(super) fn render(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<RenderPictureJobs>()
        .add_systems(RenderStartup, queue_pipeline)
        .add_systems(ExtractSchedule, extract_jobs)
        .add_systems(Render, render_jobs.in_set(RenderSystems::PrepareResources));
}

/// The main world's half: the jobs, retired once submitted, then the frame's asks laid out.
pub(super) fn jobs(app: &mut App) {
    app.init_resource::<PictureJobs>()
        .add_systems(Last, (retire, realize).chain());
}

#[cfg(test)]
pub(super) mod tests {
    use std::cell::Cell;
    use std::f32::consts::{FRAC_PI_2, TAU};
    use std::sync::atomic::AtomicUsize;

    use super::super::plan::{Ray, Thrown};
    use super::*;
    use crate::ballistics::spall_cone::FieldDescriptor;

    /// A fixture field's throw: at the line's start, at record time zero, its front at 1 m/s. These
    /// tests read a field's shape, never its timing. Fixture data.
    pub(in super::super) const AT_ORIGIN: Thrown = Thrown {
        arc: 0.0,
        t: 0.0,
        speed: 1.0,
    };

    /// A pitch near the one the shipped records derive for an expanded 4K corner (5.5·10⁻⁴ rad,
    /// MEASURED over their sightings). Fixture data.
    const PRODUCTION_PITCH: f32 = 5.5e-4;

    /// The detail the tests lay a record's pictures out at: a 4K target's ([`detail_of`]).
    /// Fixture data.
    pub(in super::super) const DETAIL: UVec2 = UVec2::new(3840, 2176);

    /// The fixtures' axis and its frame, the picture's own.
    pub(in super::super) const AXIS: Vec3 = Vec3::NEG_Z;

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

    /// A box from `lo` to `hi` added to `bodies`: 12 triangles wound outward.
    fn cuboid(lo: Vec3, hi: Vec3, bodies: &mut (Vec<Vec3>, Vec<[u32; 3]>)) {
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
            }
        }
    }

    /// The casters of a wall 3 m down [`AXIS`] and a plate `thickness` (m) thick 1 m down over the
    /// cap's `+u` half, the frame's `u` being `+x`. Fixture data.
    pub(in super::super) fn plate_casters(thickness: f32) -> Casters {
        let (u, v) = AXIS.any_orthonormal_pair();
        assert_eq!(
            (u, v),
            (Vec3::X, Vec3::NEG_Y),
            "the fixture's sides are its frame's"
        );
        let mut parts = (Vec::new(), Vec::new());
        cuboid(
            Vec3::new(-5.0, -5.0, -3.2),
            Vec3::new(5.0, 5.0, -3.0),
            &mut parts,
        );
        cuboid(
            Vec3::new(0.0, -5.0, -1.0 - thickness),
            Vec3::new(5.0, 5.0, -1.0),
            &mut parts,
        );
        Casters::of(&Arc::new(HardBodies::new(parts.0, parts.1))).expect("the fixture has vertices")
    }

    /// Each face's frame is the one [`face_of`] reads: a direction built at tangent `(a, b)` on face
    /// `k` from [`FRAMES`] comes back on face `k` at `(a, b)`, and every frame is right-handed
    /// the way the clip needs it (`right × up = −forward`). The GPU renders each face through its
    /// frame and the look reads it through `face_of`; a frame and a reader that disagree draw
    /// one face's reaches in another's directions. Fixture: a grid of tangents short of the
    /// faces' edges, where two faces tie.
    #[test]
    fn each_face_reads_back_through_its_frame() {
        for (k, [right, up, forward]) in FRAMES.into_iter().enumerate() {
            assert_eq!(right.cross(up), -forward, "face {k}");
            for i in 0..9 {
                for j in 0..9 {
                    let t = Vec2::new(i as f32 - 4.0, j as f32 - 4.0) * 0.24;
                    let local = (forward + right * t.x + up * t.y).normalize();
                    let (face, back) = face_of(local);
                    assert_eq!(face, k, "{t}");
                    assert!((back - t).length() < 1e-5, "face {k}: {back} against {t}");
                }
            }
        }
    }

    /// The vertex clip `pen_cam_picture.wgsl` takes a world point through for face uniform
    /// `face`: tangent `(x, y)` across the crop's rectangle onto `[−1, 1]`, the near plane at
    /// [`NEAR_M`], `w` the depth along the face's forward.
    fn clip(face: &FaceUniform, position: Vec3) -> Vec4 {
        let q = position - face.origin.truncate();
        let (x, y, z) = (
            q.dot(face.right.truncate()),
            q.dot(face.up.truncate()),
            q.dot(face.forward.truncate()),
        );
        let r = face.rect;
        Vec4::new(
            (2.0 * x - (r.x + r.z) * z) / (r.z - r.x),
            (2.0 * y - (r.y + r.w) * z) / (r.w - r.y),
            NEAR_M,
            z,
        )
    }

    /// A caster face the line from the origin meets head-on is kept and one it meets from behind
    /// culled: a triangle wound outward (as every hard body is) and facing the origin projects
    /// counter-clockwise through every face's clip, the pipeline's front face, and the pipeline
    /// culls the back faces; a horizon triangle wound inward does too from inside the box. The
    /// picture ends where a line enters a hard body, so a field's direction leaves the body it
    /// starts in; a frame of the other hand, a clockwise front face or a front cull draws a body's
    /// far faces and the field stops behind each body instead. Fixture: a triangle 2 m out on each
    /// face's forward, a field's frame off the world's axes.
    #[test]
    fn a_face_toward_the_origin_is_drawn_and_one_away_culled() {
        assert_eq!(caster_primitive().front_face, FrontFace::Ccw);
        assert_eq!(caster_primitive().cull_mode, CASTER_CULL);
        let mut cone = cap(|_, _| 2.0);
        cone.axis = Vec3::new(0.3, -0.5, 0.8).normalize();
        cone.origin = Vec3::new(0.2, 0.1, -0.4);
        let picture = Picture::of(&cone, 0.01);
        for face in 0..6 {
            let mut uniform = FaceUniform::of(&picture, face);
            uniform.rect = Vec4::new(-1.0, -1.0, 1.0, 1.0);
            let forward = uniform.forward.truncate();
            let (right, up) = (uniform.right.truncate(), uniform.up.truncate());
            let centre = picture.origin + forward * 2.0;
            // Wound so its normal points back at the origin.
            let toward = [centre, centre + right * 0.1, centre + up * 0.1];
            let normal = (toward[1] - toward[0]).cross(toward[2] - toward[0]);
            assert!(normal.dot(forward) < 0.0, "face {face}");
            let ndc = toward.map(|p| {
                let c = clip(&uniform, p);
                Vec2::new(c.x, c.y) / c.w
            });
            let area = (ndc[1] - ndc[0]).perp_dot(ndc[2] - ndc[0]);
            assert!(
                area > 0.0,
                "face {face}: a face toward the origin turns {area}"
            );
        }
        // The horizon from inside: every triangle faces the box's centre.
        let casters = plate_casters(0.03);
        let (vertices, indices) = casters.mesh();
        let centre = 0.5 * (casters.lo + casters.hi);
        let horizon = &indices[indices.len() - 36..];
        for t in horizon.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|k| vertices[t[k] as usize]);
            let normal = (b - a).cross(c - a);
            assert!(normal.dot(centre - a) > 0.0, "a horizon triangle faces out");
        }
    }

    /// A line from inside a body leaves it: from a point inside the plate, a direction out of it
    /// ends on the wall beyond, never on the plate's own far face, as the GPU draws with
    /// [`CASTER_CULL`] and as a line enters a hard body ([`HardBodies`]); and a direction that meets
    /// nothing ends on the horizon. A picture of two-sided casters stops every field at the
    /// first face it meets, the far face of the plate a spall cone is thrown from included.
    /// Fixture: [`plate_casters`], 3 cm thick; a point 1 cm into it.
    #[test]
    fn a_line_from_inside_a_body_leaves_it() {
        let casters = plate_casters(0.03);
        let inside = Vec3::new(1.0, 0.0, -1.01);
        for k in 0..12 {
            let d = direction(0.3, k as f32 * TAU / 12.0);
            let wall = (3.0 - 1.01) / d.dot(AXIS);
            let got = casters.reach(inside, AXIS, d);
            assert!(
                (got - wall).abs() < 1e-4,
                "{d}: {got} against the wall's {wall}"
            );
        }
        let up = casters.reach(inside, AXIS, Vec3::Z);
        assert!((up - 0.01).abs() < 1e-4, "{up}: the horizon's top");
    }

    /// The pictures' CPU mirror agrees with every recorded direction of every shipped record:
    /// along each direction a fragment ended off its field's origin, the reach the casters give
    /// ([`Casters::reach`], the GPU's clip and cull rule computed on the CPU) is the fragment's
    /// distance from the origin, within 10⁻⁴ m. A field whose every fragment ended at its origin
    /// is the record-wins case and is skipped here
    /// (`a_field_the_record_stops_at_its_origin_draws_nothing`). Casters other than the record's
    /// hard bodies (crew stations cast as hard bodies stop the fields that run through them) fail
    /// here. This holds the mirror alone: no test reads the GPU's pictures back.
    #[test]
    fn the_cpu_mirror_agrees_with_every_recorded_direction() {
        let (mut checked, mut fields) = (0, 0);
        for (name, report) in super::super::plan::tests::records() {
            let shapes = super::super::plan::tests::shapes(&report);
            let Some(bodies) = shapes.hard() else {
                continue;
            };
            let casters = Casters::of(bodies).expect("the bodies have vertices");
            for (k, field) in report.fields().enumerate() {
                let origin = Vec3::from(field.origin);
                let mut ended = 0;
                for fragment in &field.fragments {
                    let span = Vec3::from(fragment.end) - origin;
                    let Some(d) = span.try_normalize() else {
                        continue;
                    };
                    let axis = Vec3::from(field.axis).normalize_or(Vec3::NEG_Z);
                    let (want, got) = (span.length(), casters.reach(origin, axis, d));
                    assert!(
                        (got - want).abs() <= 1e-4,
                        "{name} field {k}: {got} m against the record's {want} m along {d}"
                    );
                    ended += 1;
                }
                checked += ended;
                fields += usize::from(ended > 0);
            }
        }
        // The shipped library checks 1408 directions over 19 fields (MEASURED): a floor under that,
        // so a library that stops reaching the mirror fails here.
        assert!(checked > 1300 && fields > 17, "{checked} {fields}");
    }

    /// The near plane lies [`NEAR_M`] along each face's forward, as `pen_cam_picture.wgsl` clips:
    /// a direction 40° off the axis lies on the axis's face and starts `NEAR_M / cos 40°` out, so
    /// a plate whose near side lies 0.9 `NEAR_M` down the axis is passed (the line starts inside
    /// it and leaves it unstopped) and one 1.1 `NEAR_M` down stops it at that side. A mirror that
    /// starts `NEAR_M` out along the direction stops on the first plate, where the GPU's picture
    /// runs on to the wall. Fixture: a plate 1 cm thick across the axis over a wall 3 m down.
    #[test]
    fn the_near_plane_lies_along_the_faces_forward() {
        let d = direction(40.0_f32.to_radians(), 0.0);
        let depth = d.dot(AXIS);
        for (near_side, stopped) in [(0.9 * NEAR_M, false), (1.1 * NEAR_M, true)] {
            let mut parts = (Vec::new(), Vec::new());
            cuboid(
                Vec3::new(-5.0, -5.0, -3.2),
                Vec3::new(5.0, 5.0, -3.0),
                &mut parts,
            );
            cuboid(
                Vec3::new(-1.0, -1.0, -near_side - 0.01),
                Vec3::new(1.0, 1.0, -near_side),
                &mut parts,
            );
            let bodies = Arc::new(HardBodies::new(parts.0, parts.1));
            let casters = Casters::of(&bodies).expect("the fixture has vertices");
            let want = if stopped { near_side } else { 3.0 } / depth;
            let got = casters.reach(Vec3::ZERO, AXIS, d);
            assert!(
                (got - want).abs() <= 1e-3 * want,
                "plate {near_side} m down: {got} m against {want} m"
            );
        }
    }

    /// Every face's crop holds the support with the texels a read on it weighs: for every field of
    /// every shipped record at its own pitch and for the fixtures' cap, band and sphere, every
    /// direction on the support lies on a face whose crop holds the four texels about it. A crop
    /// cut short of the support draws the field's edge as nothing.
    #[test]
    fn every_crop_holds_its_support() {
        let mut cones: Vec<(Cone, f32)> = [cap(|_, _| 1.0), band(|_, _| 1.0), sphere(|_, _| 1.0)]
            .into_iter()
            .map(|cone| (cone, 4e-3))
            .collect();
        for (_, report) in super::super::plan::tests::records() {
            let shapes = super::super::plan::tests::shapes(&report);
            let plan = super::super::plan::Plan::new(&report, &shapes);
            let pictures = plan.lay()(&[DETAIL]);
            let pitches: Vec<f32> = pictures
                .fields
                .iter()
                .map(|picture| picture.pitch)
                .collect();
            cones.extend(plan.cones.into_iter().zip(pitches));
        }
        let mut checked = 0;
        for (cone, pitch) in &cones {
            if cone.rays.is_empty() {
                continue;
            }
            let picture = Picture::of(cone, *pitch);
            for k in 0..400 {
                let theta = picture.inner
                    + (picture.half_angle.min(PI) - picture.inner) * (k as f32 + 0.5) / 400.0;
                for j in 0..36 {
                    let local = Vec3::new(
                        theta.sin() * (j as f32 * TAU / 36.0).cos(),
                        theta.sin() * (j as f32 * TAU / 36.0).sin(),
                        theta.cos(),
                    );
                    let (face, t) = face_of(local);
                    let crop = picture.crops[face];
                    let span = crop.size.as_vec2() * picture.pitch;
                    let g = Vec2::new(t.x - crop.x0, crop.y1 - t.y) / picture.pitch - 0.5;
                    assert!(
                        crop.size.min_element() >= 2
                            && g.min_element() >= 0.0
                            && g.x <= span.x / picture.pitch - 1.0
                            && g.y <= span.y / picture.pitch - 1.0,
                        "θ {theta}: face {face} at {t}, crop {crop:?}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 100_000, "{checked}");
    }

    /// The atlas places every face apart and inside it: no two crops of a record's pictures
    /// overlap, every crop lies within the atlas, and no side passes [`MAX_SIDE`]. Faces that
    /// overlap draw one face's reaches over another's. Fixture: every shipped record's pictures,
    /// and a set of sizes that must wrap onto several rows.
    #[test]
    fn the_atlas_places_every_face_apart() {
        let mut sets: Vec<Vec<UVec2>> = vec![
            (0..24)
                .map(|k| UVec2::new(300 + 37 * (k % 7), 200 + 53 * (k % 5)))
                .collect(),
        ];
        for (_, report) in super::super::plan::tests::records() {
            let shapes = super::super::plan::tests::shapes(&report);
            let plan = super::super::plan::Plan::new(&report, &shapes);
            let pictures = plan.lay()(&[DETAIL]);
            let crops: Vec<Crop> = pictures.fields.iter().flat_map(|f| f.crops).collect();
            let rects: Vec<(UVec2, UVec2)> = crops
                .iter()
                .filter(|c| c.size.min_element() > 0)
                .map(|c| (c.place, c.place + c.size))
                .collect();
            check_apart(&rects, pictures.size);
            sets.push(crops.iter().map(|c| c.size).collect());
        }
        for sizes in sets {
            let (places, size) = pack(&sizes).expect("they fit");
            let rects: Vec<(UVec2, UVec2)> = places
                .iter()
                .zip(&sizes)
                .filter(|(_, s)| s.min_element() > 0)
                .map(|(&p, &s)| (p, p + s))
                .collect();
            check_apart(&rects, size);
        }
    }

    fn check_apart(rects: &[(UVec2, UVec2)], size: UVec2) {
        assert!(size.max_element() <= MAX_SIDE, "{size}");
        for (k, &(lo, hi)) in rects.iter().enumerate() {
            assert!(hi.x <= size.x && hi.y <= size.y, "{lo} {hi} past {size}");
            for &(a, b) in &rects[k + 1..] {
                let apart = hi.x <= a.x || b.x <= lo.x || hi.y <= a.y || b.y <= lo.y;
                assert!(apart, "{lo}..{hi} overlaps {a}..{b}");
            }
        }
    }

    /// A surface the field stops on reads inside across it: on a plane every direction of a cap
    /// stops on, each texel reaching the plane along its own direction, a point of the plane reads
    /// the field whole at every direction swept across it, though about most of them a texel whose
    /// direction meets the plane nearer leaves the point past that texel's own reach. The volume's
    /// march ends on a component's surface and the painting lies on it; a read with no tolerance
    /// ([`REACH_TOLERANCE_DIAGONALS`]) flips the last stretch before the surface texel by texel, a
    /// stair-step, and a tolerance not grown with the distance flips it on a surface far out.
    /// Fixture: the 25° cap at [`PRODUCTION_PITCH`]; the plane 3 m out, turned 45° about `v`, so the
    /// swept directions, up to 20° off the axis, meet it from 20° to 60° off square and 3 to 6 m
    /// out, short of `tan⁻¹ 2` = 63°, the steepest the tolerance holds at a face's centre (DERIVED).
    #[test]
    fn a_surface_the_field_stops_on_reads_inside_across_it() {
        let (u, _) = AXIS.any_orthonormal_pair();
        let tilt = 45.0_f32.to_radians();
        // The plane's unit normal away from the origin, and its distance from it.
        let away = AXIS * tilt.cos() + u * tilt.sin();
        let distance = 3.0;
        let plane = |d: Vec3| distance / d.dot(away);
        let picture = Picture::of(&cap(|_, _| 1.0), PRODUCTION_PITCH);
        let full = 1.0;
        let (mut checked, mut stepped) = (0, 0);
        for k in 0..60 {
            for j in 0..48 {
                let d = direction((k as f32 / 3.0).to_radians(), j as f32 * TAU / 48.0 + 0.1);
                if d.dot(away).acos() > 60.0_f32.to_radians() {
                    continue;
                }
                let r = plane(d);
                let got = picture.lit(picture.local(d), r, 1.0, 0.0, &plane);
                assert!(
                    (got - full).abs() <= 1e-5 * full,
                    "{d} at {r} m: {got} of {full}"
                );
                let short = Cell::new(false);
                picture.lit(picture.local(d), r, 1.0, 0.0, &|t| {
                    short.set(short.get() || plane(t) < r);
                    plane(t)
                });
                stepped += usize::from(short.get());
                checked += 1;
            }
        }
        assert!(
            checked > 2000 && 2 * stepped > checked,
            "{checked} {stepped}"
        );
    }

    /// A point a body's thickness behind a blocker stays outside: behind a plate 3 mm thick 1 m
    /// out, a point on the plate's far face reads the field as one before the wall behind it does,
    /// the open texels' share alone, at every direction across the plate's silhouette; and 1 mm
    /// into the plate, where every texel about it ends on its near face, a point reads nothing. A
    /// blocked texel reads past its reach by at most [`REACH_TOLERANCE_DIAGONALS`] diagonals times
    /// the distance (under 1.7 mm here, DERIVED) and never past the farthest of the four about a
    /// direction, so a shadow holds behind a body thicker than that; a tolerance twice as wide
    /// (3.3 mm) carries the field onto the far face beside the silhouette, and one reaching past
    /// the farthest of the four carries it into the plate. Fixture: [`plate_casters`] at
    /// [`PRODUCTION_PITCH`]; directions 8°, 14° and 20° off the axis swept a degree either side of
    /// the plate's edge.
    #[test]
    fn a_point_a_bodys_thickness_behind_a_blocker_stays_outside() {
        let thickness = 0.003;
        let casters = plate_casters(thickness);
        let reach = |d: Vec3| casters.reach(Vec3::ZERO, AXIS, d);
        let picture = Picture::of(&cap(|_, d| reach(d)), PRODUCTION_PITCH);
        let full = 1.0;
        let (mut across, mut within, mut shared) = (0, 0, 0);
        for k in 0..=1000 {
            let off = (k as f32 - 500.0) * 0.002_f32.to_radians();
            for theta in [8.0_f32, 14.0, 20.0] {
                let d = direction(
                    theta.to_radians(),
                    FRAC_PI_2 + off / theta.to_radians().sin(),
                );
                if d.x <= 0.0 {
                    continue;
                }
                let depth = d.dot(AXIS);
                let local = picture.local(d);
                let far_face = picture.lit(local, (1.0 + thickness) / depth, 1.0, 0.0, &reach);
                let before_wall = picture.lit(local, 2.9 / depth, 1.0, 0.0, &reach);
                assert!(
                    (far_face - before_wall).abs() <= 1e-5 * full,
                    "{d}: {far_face} on the plate's far face, {before_wall} before the wall"
                );
                shared += usize::from(far_face > 0.0 && far_face < full);
                across += 1;
                if d.x / depth >= 1.5 * PRODUCTION_PITCH {
                    let inside = picture.lit(local, 1.001 / depth, 1.0, 0.0, &reach);
                    assert_eq!(inside, 0.0, "{d}: 1 mm into the plate");
                    within += 1;
                }
            }
        }
        assert!(
            across > 1400 && within > 1300 && shared > 10,
            "{across} {within} {shared}"
        );
    }

    /// The pictures of [`cap`] out to `reach` (m) over [`plate_casters`], at no sighting.
    fn capped(reach: f32) -> Arc<Pictures> {
        let casters = plate_casters(0.003);
        Arc::new(Pictures::of(
            &[cap(|_, _| reach)],
            Some(&casters.bodies),
            &[],
            &[],
        ))
    }

    /// A record's pictures keep one atlas while any view holds it: every ask reads the one image
    /// and queues one job; once every holder lets it go and its job leaves, the next ask makes and
    /// queues it anew. The atlas is the pass's render target and the look's texture at the packed
    /// size, its texels left to the GPU. An atlas made per ask renders the record again for each
    /// frame that draws it. Fixture: [`cap`] over [`plate_casters`].
    #[test]
    fn the_atlas_is_cached_while_a_view_holds_it() {
        let pictures = capped(2.0);
        let mut images = Assets::<Image>::default();
        let mut jobs = PictureJobs::default();
        let first = pictures.image(&mut images, &mut jobs);
        let again = pictures.image(&mut images, &mut jobs);
        assert!(first.is_some(), "a field over casters is drawn");
        assert_eq!(first, again, "a second ask reads another atlas");
        assert_eq!(jobs.0.len(), 1, "a second ask renders the pictures again");
        let image = first.as_ref().and_then(|atlas| images.get(atlas));
        let image = image.expect("the atlas is stored");
        assert_eq!(image.size(), pictures.size);
        assert_eq!(image.texture_descriptor.format, TextureFormat::R32Float);
        let usage = TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING;
        assert!(image.texture_descriptor.usage.contains(usage));
        assert!(image.data.is_none(), "the CPU writes the atlas's texels");
        drop((first, again));
        jobs.retire();
        assert!(pictures.image(&mut images, &mut jobs).is_some());
        assert_eq!(jobs.0.len(), 1, "an atlas no one holds is not made anew");
    }

    /// A picture's job waits until submitted, is taken once, and leaves after: until the render
    /// world submits it the atlas is not [`PictureJobs::rendered`] and the job is among those it
    /// takes each frame ([`PictureJobs::waiting`]); once submitted the atlas is rendered, the job
    /// is taken no more and retirement drops it. A job whose atlas no view holds any more leaves
    /// unsubmitted. A view that reads an atlas before its job is submitted draws an empty picture;
    /// a job taken after its submission renders the record again. The render world's half
    /// (`render_jobs`, `encode`) needs a GPU and is not run here: [`PictureJobs::submit_all`]
    /// stands in for it. Fixture: [`cap`] over [`plate_casters`].
    #[test]
    fn a_job_waits_until_submitted_and_leaves_after() {
        let pictures = capped(2.0);
        let mut images = Assets::<Image>::default();
        let mut jobs = PictureJobs::default();
        assert!(jobs.rendered(None), "no atlas waits on a job");
        let atlas = pictures.image(&mut images, &mut jobs);
        assert!(atlas.is_some(), "a field over casters is drawn");
        assert!(
            !jobs.rendered(atlas.as_ref()),
            "the atlas reads before it is submitted"
        );
        jobs.retire();
        assert_eq!(jobs.waiting().count(), 1, "a held job leaves unsubmitted");
        jobs.submit_all();
        assert!(
            jobs.rendered(atlas.as_ref()),
            "a submitted atlas still waits"
        );
        assert_eq!(jobs.waiting().count(), 0, "a submitted job is taken again");
        jobs.retire();
        assert!(jobs.0.is_empty(), "a submitted job stays");
        assert!(jobs.rendered(atlas.as_ref()));

        let replaced = capped(1.5);
        let pending = replaced.image(&mut images, &mut jobs);
        assert_eq!(jobs.waiting().count(), 1);
        drop(pending);
        jobs.retire();
        assert!(jobs.0.is_empty(), "a job no view holds is rendered");
    }

    /// A texel spans at most a pixel of the live target at the sightings, and no less: for every
    /// field of every shipped record, a 1080p target's pitch is twice a 4K target's (their details
    /// are of one aspect, one twice the other), [`COARSEST_PITCH`] at most, and its pictures hold a
    /// quarter of the 4K pictures' texels and the little more the texel each crop runs past its
    /// face's edge adds (MEASURED: the 4K pictures hold 3.985 times the 1080p ones'). A pitch
    /// fixed at one reference size gives a 1080p target four times the texels it can show, or a 4K
    /// target a texel of two pixels. Fixture: every shipped record.
    #[test]
    fn the_pitch_follows_the_targets_size() {
        let (hd, uhd) = (
            detail_of(UVec2::new(1920, 1080)),
            detail_of(UVec2::new(3840, 2160)),
        );
        assert_eq!(uhd, 2 * hd, "the fixture's targets are not of one aspect");
        let (mut fields, mut texels) = (0, [0_u64; 2]);
        for (name, report) in super::super::plan::tests::records() {
            let shapes = super::super::plan::tests::shapes(&report);
            let plan = super::super::plan::Plan::new(&report, &shapes);
            let sightings = super::super::stage::field_sightings(&plan);
            for (k, cone) in plan.cones.iter().enumerate() {
                if cone.rays.is_empty() {
                    continue;
                }
                let [coarse, fine] = [hd, uhd].map(|detail| pitch_of(cone, &sightings, detail));
                let want = (2.0 * fine).min(COARSEST_PITCH);
                assert!(
                    (coarse - want).abs() <= 1e-6 * want,
                    "{name} field {k}: {coarse} rad at 1080p, {fine} rad at 4K"
                );
                if coarse < COARSEST_PITCH {
                    texels[0] += Picture::of(cone, coarse).texels();
                    texels[1] += Picture::of(cone, fine).texels();
                    fields += 1;
                }
            }
        }
        let ratio = texels[1] as f64 / texels[0] as f64;
        assert!(
            fields > 0 && (3.9..=4.0).contains(&ratio),
            "{fields} fields: 4K holds {ratio} times the 1080p texels"
        );
    }

    /// The layout of `cone` over `casters` ([`Lay`]), each laying out counted in `lays`.
    fn counted(
        cone: &Cone,
        casters: &Casters,
        lays: &Arc<AtomicUsize>,
    ) -> impl Fn() -> Lay + use<> {
        let (cone, bodies, lays) = (cone.clone(), casters.bodies.clone(), lays.clone());
        move || {
            let (cone, bodies, lays) = (cone.clone(), bodies.clone(), lays.clone());
            Box::new(move |details: &[UVec2]| {
                lays.fetch_add(1, Ordering::Relaxed);
                Pictures::of(std::slice::from_ref(&cone), Some(&bodies), &[], details)
            })
        }
    }

    /// A frame's end as [`jobs`] runs it: the jobs retired, then what the views asked laid out.
    fn end_frame(images: &mut Assets<Image>, jobs: &mut PictureJobs) {
        jobs.retire();
        jobs.realize(images);
    }

    /// A silhouette across a face's edge is drawn from the two faces' own grids, never a blend of
    /// them: on either side, out to the edge, the read's half-lit contour lies within half a texel
    /// of the silhouette, across it in that face's tangent grid (DERIVED: four texels weighted
    /// bilinearly put a step's half-way at most half a texel off it), so the silhouette jogs at
    /// the edge by at most a texel. [`Picture::lit`] relies on the direction's own face
    /// ([`face_of`]) and on its crop running a texel past the face's edge ([`Crop::of`]): a read
    /// from another face's grid, or one clamped at its crop's edge, strays further. Fixture: a
    /// whole-sphere field cut by a great circle, 6,000 seeded trials over every edge of the cube
    /// from either side, the circle crossing the edge at 10° to 170°, read on a line along the
    /// edge within a texel of it, at pitches about the production one (which set each grid's
    /// phase against the edge).
    #[test]
    fn a_silhouette_across_a_faces_edge_jogs_by_at_most_a_texel() {
        let cone = sphere(|_, _| 1.0);
        let cells = Picture::cells(&cone);
        let edges: Vec<(usize, usize)> = (0..6)
            .flat_map(|face| (0..6).map(move |other| (face, other)))
            .filter(|&(face, other)| FRAMES[face][2].dot(FRAMES[other][2]) == 0.0)
            .collect();
        assert_eq!(edges.len(), 24, "every edge, from either side");
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        let mut uniform = move |low: f32, high: f32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            low + (high - low) * (seed >> 40) as f32 / (1_u64 << 24) as f32
        };
        for trial in 0..6000 {
            let pitch = uniform(0.4e-3, 0.7e-3);
            let picture = Picture::with_cells(&cone, &cells, pitch);
            let (f, g) = edges[(uniform(0.0, 24.0) as usize).min(23)];
            let along = FRAMES[f][2].cross(FRAMES[g][2]);
            let w0 = uniform(-0.9, 0.9);
            let s = (FRAMES[f][2] + FRAMES[g][2] + along * w0).normalize();
            let edge = along.reject_from(s).normalize();
            let psi = uniform(10.0, 170.0).to_radians();
            let n = s
                .cross(edge.cross(s) * psi.sin() + edge * psi.cos())
                .normalize();
            let blocked = |direction: Vec3| picture.local(direction).dot(n) > 0.0;
            let reach = |direction: Vec3| if blocked(direction) { 0.5 } else { 5.0 };
            let depth = uniform(1e-3, 1.0) * pitch;
            let window = 8.0 * pitch * (2.0 + w0 * w0) / SQRT_2;
            for (face, other) in [(f, g), (g, f)] {
                let [right, up, forward] = FRAMES[face];
                let point =
                    |w: f32| (forward + FRAMES[other][2] * (1.0 - depth) + along * w).normalize();
                let half = |w: f32| picture.lit(point(w), 1.0, 1.0, 0.0, &reach) - 0.5;
                let ws: Vec<f32> = (0..=320)
                    .map(|k| w0 - window + 2.0 * window * k as f32 / 320.0)
                    .collect();
                let crossing = ws.windows(2).find(|w| {
                    let (a, b) = (half(w[0]), half(w[1]));
                    a * b <= 0.0 && a != b
                });
                let Some(bracket) = crossing else {
                    panic!("trial {trial}: no silhouette on face {face} by {other}");
                };
                let (mut lo, mut hi) = (bracket[0], bracket[1]);
                let below = half(lo);
                for _ in 0..30 {
                    let mid = 0.5 * (lo + hi);
                    if half(mid) * below > 0.0 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let on = point(0.5 * (lo + hi));
                let tangent = on / on.dot(forward);
                let off = n.dot(tangent) / n.dot(right).hypot(n.dot(up)) / pitch;
                assert!(
                    off.abs() <= 0.5 + 1e-3,
                    "trial {trial}: face {face} by {other} draws the silhouette {off} texels off"
                );
            }
        }
    }

    /// A target that changes within its step lays nothing anew: a view whose target moves about
    /// within one step of [`DETAIL_STEP_PX`] keeps the pictures laid out and rendered for its
    /// first size, and a change across a step lays them out once more. A view that laid its
    /// record's pictures out anew at every change of size renders the record again on every frame
    /// of a window's drag. Fixture: [`cap`] over [`plate_casters`], targets about 1900×1000.
    #[test]
    fn a_change_of_size_within_a_step_lays_nothing_anew() {
        let lays = Arc::new(AtomicUsize::new(0));
        let lay = counted(&cap(|_, _| 2.0), &plate_casters(0.003), &lays);
        let record = Arc::new(RecordAtlas::default());
        let mut view = ViewPictures::default();
        let (mut images, mut jobs) = (Assets::<Image>::default(), PictureJobs::default());
        let detail = detail_of(UVec2::new(1900, 1000));
        record.show(&mut view, detail, &lay, &mut images, &mut jobs);
        end_frame(&mut images, &mut jobs);
        jobs.submit_all();
        for (w, h) in [(1901, 1001), (1919, 1023), (1880, 990), (1900, 1000)] {
            let detail = detail_of(UVec2::new(w, h));
            record.show(&mut view, detail, &lay, &mut images, &mut jobs);
            end_frame(&mut images, &mut jobs);
            let laid = lays.load(Ordering::Relaxed);
            assert_eq!(laid, 1, "{w}×{h} laid the pictures out anew");
            assert_eq!(jobs.queued(), 0, "{w}×{h} rendered the pictures anew");
            assert!(view.drawn().is_some(), "{w}×{h} drew no pictures");
        }
        let detail = detail_of(UVec2::new(1921, 1000));
        record.show(&mut view, detail, &lay, &mut images, &mut jobs);
        end_frame(&mut images, &mut jobs);
        let laid = lays.load(Ordering::Relaxed);
        assert_eq!(laid, 2, "a change across a step laid nothing anew");
        assert_eq!(
            jobs.queued(),
            1,
            "a change across a step rendered nothing anew"
        );
    }

    /// Pictures laid out anew are drawn from the atlas before them until theirs is written:
    /// across a change of size the view draws the old pictures from the old atlas in every frame
    /// until the new atlas's job is submitted, then the new from the new, and lets the old go. A
    /// record's first pictures are drawn in no frame before theirs is submitted. A view that let
    /// the old atlas go when it laid out anew draws frames without the fields at every change of
    /// size; one that drew the new pictures before submission reads an empty atlas. Fixture:
    /// [`cap`] over [`plate_casters`], a 1080p target growing to 4K.
    #[test]
    fn pictures_laid_anew_are_drawn_from_the_old_atlas_until_written() {
        let lay = counted(&cap(|_, _| 2.0), &plate_casters(0.003), &Arc::default());
        let (small, large) = (
            detail_of(UVec2::new(1920, 1080)),
            detail_of(UVec2::new(3840, 2160)),
        );
        let record = Arc::new(RecordAtlas::default());
        let mut view = ViewPictures::default();
        let (mut images, mut jobs) = (Assets::<Image>::default(), PictureJobs::default());
        let mut frame = |detail, view: &mut ViewPictures, jobs: &mut PictureJobs| {
            record.show(view, detail, &lay, &mut images, jobs);
            end_frame(&mut images, jobs);
        };
        frame(small, &mut view, &mut jobs);
        assert!(view.drawn().is_none(), "the first pictures drawn unwritten");
        jobs.submit_all();
        frame(small, &mut view, &mut jobs);
        let (old, old_atlas) = view.drawn().cloned().expect("the first pictures, written");
        for k in 0..3 {
            frame(large, &mut view, &mut jobs);
            let (pictures, atlas) = view.drawn().expect("a frame without the fields");
            assert!(
                Arc::ptr_eq(pictures, &old) && *atlas == old_atlas,
                "frame {k}: the new pictures drawn unwritten"
            );
        }
        jobs.submit_all();
        frame(large, &mut view, &mut jobs);
        let (new, new_atlas) = view.drawn().cloned().expect("the new pictures, written");
        assert_eq!(new.details, vec![large]);
        assert_ne!(new_atlas, old_atlas);
        assert!(
            !view.held.contains(&old_atlas),
            "the old atlas held past the new one's writing"
        );
    }

    /// The views of a record take the finest pitch any of them needs at its own size: two plans
    /// of one record, one asking a 1080p detail and the other a 4K one in the same frame, lay one
    /// atlas out at both, each field at the pitch the 4K detail gives, its one job the first
    /// frame's only, and nothing but that atlas held once it is written; once the 4K view lets go,
    /// the other's next frame lays the pictures out at its own detail alone. Views laying a record
    /// out each at its own size render it twice; ones laying out on each ask render the first
    /// view's layout too, superseded unseen; ones keeping a view's ask after it goes keep its
    /// texels. Fixture: the first shipped record that throws a field.
    #[test]
    fn the_views_of_a_record_take_the_finest_pitch_any_needs() {
        let (corner, mut xray) = super::super::plan::tests::records()
            .into_iter()
            .find_map(|(_, report)| {
                let shapes = super::super::plan::tests::shapes(&report);
                let plan = super::super::plan::Plan::new(&report, &shapes);
                plan.cones
                    .iter()
                    .any(|cone| !cone.rays.is_empty())
                    .then(|| (plan, super::super::plan::Plan::new(&report, &shapes)))
            })
            .expect("a record throws a field");
        xray.adopt_pictures(corner.picture_cell().clone());
        let (small, large) = (
            detail_of(UVec2::new(1920, 1080)),
            detail_of(UVec2::new(3840, 2160)),
        );
        let (mut a, mut b) = (ViewPictures::default(), ViewPictures::default());
        let (mut images, mut jobs) = (Assets::<Image>::default(), PictureJobs::default());
        for frame in 0..2 {
            corner.show(&mut a, small, &mut images, &mut jobs);
            xray.show(&mut b, large, &mut images, &mut jobs);
            end_frame(&mut images, &mut jobs);
            if frame == 0 {
                assert_eq!(jobs.queued(), 1, "a layout queued for each view");
            }
            jobs.submit_all();
        }
        let (shared, atlas) = a.drawn().cloned().expect("the corner draws the pictures");
        assert_eq!(
            b.drawn().map(|(_, atlas)| atlas),
            Some(&atlas),
            "two atlases"
        );
        assert_eq!(shared.details, vec![small, large]);
        let laying = corner
            .picture_cell()
            .0
            .lock()
            .expect("the record's pictures");
        assert!(
            laying.next.is_none() && jobs.queued() == 0,
            "a superseded layout kept"
        );
        drop(laying);
        assert!(
            [&a, &b].iter().all(|view| view.held == [atlas.clone()]),
            "an atlas held beside the one drawn"
        );
        let finest = corner.lay()(&[large]);
        let pitches = |pictures: &Pictures| -> Vec<f32> {
            pictures.fields.iter().map(|field| field.pitch).collect()
        };
        assert_eq!(pitches(&shared), pitches(&finest), "not the finest pitch");
        drop(b);
        for _ in 0..2 {
            corner.show(&mut a, small, &mut images, &mut jobs);
            end_frame(&mut images, &mut jobs);
            jobs.submit_all();
        }
        let (own, _) = a.drawn().expect("the corner draws the pictures");
        assert_eq!(own.details, vec![small], "a view's ask kept after it went");
    }
}
