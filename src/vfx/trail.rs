//! The 88 shell's smoke trail (survey trick 6): ONE camera-facing ribbon mesh per shell, built
//! from the sim's existing [`ShellPath`] recording — no new sim state. Each render frame the
//! ribbon is rebuilt on the CPU: strip vertices offset ± half-width along
//! `normalize(cross(segment_dir, to_camera))`, width growing with point age (smoke expands), a
//! small per-point lateral drift ∝ age² (wind shear), V = arc length anchored at capture time.
//! The fragment shader (`vfx_trail.wgsl`) scrolls tileable noise along the strip and ERODES alpha
//! by per-vertex point age, so the tail dissolves with detail instead of fading as a tube.
//!
//! One draw per trail, one SHARED material for all trails (per-vertex age + view-global time carry
//! all the animation, so nothing is per-instance — the batching-friendly half of the discipline the
//! per-billboard clones can't have). Point count is bounded by capture spacing
//! ([`TRAIL_MIN_SPACING`]), point lifetime, and a hard cap. 88-only, by the same
//! `ShellPath + WorldAssetRoot` signature the headless MG test pins as "main-gun shell scene".
//!
//! The trail entity OUTLIVES its shell: the shell despawns at impact, the ribbon stays and
//! dissolves point-by-point until empty, then despawns itself.

use std::collections::VecDeque;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::Image;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use bevy::world_serialization::WorldAssetRoot;

use crate::ballistics::ShellPath;

use super::ViewRng;
use super::billboard::{gradient_lut, smoothstep};

/// Minimum spacing (m) between captured trail points — the emit-every-N-meters cap. At the 88's
/// ~773 m/s that is ~55 points/s.
const TRAIL_MIN_SPACING: f32 = 14.0;
/// A trail point's lifetime (s) — the tail fully erodes by then (the shader's age axis).
const TRAIL_POINT_LIFETIME: f32 = 2.2;
/// Hard per-trail point cap; with the spacing + lifetime above, steady state is ~120.
const TRAIL_MAX_POINTS: usize = 128;
/// Ribbon width (m): at birth, plus growth per second of point age (smoke expands as it ages).
const TRAIL_WIDTH_BIRTH: f32 = 0.35;
const TRAIL_WIDTH_GROWTH: f32 = 1.3;
/// Per-point lateral drift speed bound (m/s at age 1 — applied ∝ age², so it reads as wind taking
/// the old smoke, not the fresh line wobbling) plus a constant gentle rise.
const TRAIL_DRIFT_LATERAL: f32 = 0.4;
const TRAIL_DRIFT_RISE: f32 = 0.35;
/// Live-trail ring cap (the 88 reloads in 3 s and trails outlive shells by ~2 s, so >2 live trails
/// per gun is already a refire storm).
const TRAIL_CAP: usize = 8;
/// Arc length (m) from the muzzle below which the ribbon emits NO geometry at all. Looking down the
/// trail axis from the gunner's seat, the near-muzzle quads overlap in screen space and their
/// individually-low alphas stack back-to-front — semi-transparent geometry at the barrel still
/// reads as smoke at the barrel. The fix is honest absence: the shell's trail only begins a few
/// metres out (real behaviour — propellant smoke AT the muzzle belongs to the muzzle PUFF, which
/// exists and lives ~1.2 s). The ribbon's near end is pinned at exactly this arc by interpolating a
/// boundary station onto it, so it doesn't pop by a station as points expire.
const TRAIL_START_ARC: f32 = 10.0;
/// Arc length (m) over which the trail fades IN (width AND alpha) past [`TRAIL_START_ARC`]: the near
/// end is a pinched-to-nothing wisp at the start arc, easing to full over this stretch — a taper,
/// not a hard tube cap. The muzzle puff masks the gap between barrel and start arc.
const HEAD_FADE_ARC: f32 = 20.0;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TrailRing>()
        .add_plugins(MaterialPlugin::<VfxTrailMaterial>::default())
        .add_systems(Startup, setup_trail_assets)
        .add_systems(Update, (attach_trails, update_trails).chain());
}

/// The trail ribbon material (fragment: `vfx_trail.wgsl`): scrolled tileable noise, per-vertex age
/// erosion, gradient-map coloring. ONE instance serves every trail (see the module doc).
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(crate) struct VfxTrailMaterial {
    #[uniform(0)]
    pub params: TrailParams,
    #[texture(1)]
    #[sampler(2)]
    pub noise: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    pub lut: Handle<Image>,
}

/// The static uniform block `vfx_trail.wgsl` reads (lane map in the shader).
#[derive(ShaderType, Debug, Clone)]
pub(crate) struct TrailParams {
    /// x: noise scale along the trail (1/m), y: scroll speed, z: erosion sharpness, w: alpha.
    pub shape: Vec4,
    /// x: emissive boost at LUT heat 1.0, y: erosion floor at age 0.
    pub glow: Vec4,
}

impl Material for VfxTrailMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/vfx_trail.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        // Smoke is mass: alpha-blend, never additive (dark smoke on additive disappears).
        AlphaMode::Blend
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }
}

/// Preloaded trail view assets: the one shared material.
#[derive(Resource)]
pub(super) struct TrailAssets {
    pub(super) material: Handle<VfxTrailMaterial>,
}

pub(super) fn setup_trail_assets(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<VfxTrailMaterial>>,
) {
    let noise = noise_texture(&mut images);
    // Powder smoke: warm gray young trail cooling toward a pale, slightly blue dissipation; a
    // whisper of heat right at the fresh end so the first meters catch a little bloom.
    let lut = gradient_lut(&mut images, |x, y| {
        let lum = 0.30 + 0.45 * x;
        let warm = (1.0 - y) * 0.18;
        let color = LinearRgba::rgb(
            lum * (0.92 + warm),
            lum * (0.90 + warm * 0.5),
            lum * (0.88 + (y * 0.08)),
        );
        let heat = 0.35 * x * (-y * 12.0).exp();
        (color, heat)
    });
    let material = materials.add(VfxTrailMaterial {
        params: TrailParams {
            shape: Vec4::new(1.0 / 26.0, 0.05, 3.2, 0.55),
            glow: Vec4::new(4.0, 0.18, 0.0, 0.0),
        },
        noise,
        lut,
    });
    commands.insert_resource(TrailAssets { material });
}

/// Marker on a shell whose trail has been attached (so `attach_trails` runs once per shell).
#[derive(Component)]
struct TrailedShell;

/// One captured trail point (view data; the sim's `ShellPath` is only ever read).
pub(crate) struct TrailPoint {
    pub pos: Vec3,
    pub age: f32,
    /// Fixed world-space drift direction × magnitude; applied ∝ age² at mesh build.
    pub drift: Vec3,
    /// Arc length from the muzzle at CAPTURE time — the stable V coordinate (expiring tail points
    /// must not make the noise pattern swim along the trail).
    pub arc: f32,
    /// Per-point random seed for the shader's noise row.
    pub seed: f32,
}

/// A live smoke ribbon following (then outliving) one shell.
#[derive(Component)]
pub(crate) struct TrailRibbon {
    shell: Entity,
    points: VecDeque<TrailPoint>,
    /// `ShellPath::points` consumed so far (the recording only appends).
    consumed: usize,
    /// Running arc length at the last captured point.
    arc: f32,
}

/// Live trails, oldest first — the refire leak bound (see [`TRAIL_CAP`]).
#[derive(Resource, Default)]
struct TrailRing(VecDeque<Entity>);

/// Give every new main-gun shell a trail ribbon. The gate is the same signature the headless MG
/// test pins for "88 shell": a `ShellPath` (a shell in flight) carrying the `shell.glb` scene root
/// (`WorldAssetRoot` — only the main-gun branch of `ballistics::on_fire_shell` attaches one).
fn attach_trails(
    shells: Query<(Entity, &ShellPath), (With<WorldAssetRoot>, Without<TrailedShell>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    assets: Res<TrailAssets>,
    mut ring: ResMut<TrailRing>,
    mut rng: ResMut<ViewRng>,
    mut commands: Commands,
) {
    for (shell, path) in &shells {
        commands.entity(shell).insert(TrailedShell);
        let mut ribbon = TrailRibbon {
            shell,
            points: VecDeque::new(),
            consumed: 0,
            arc: 0.0,
        };
        // A net catch-up shell spawns with its skipped flight already in `ShellPath` — capture it
        // now with HONEST ages (one fixed tick per recorded point, oldest first), so a remote
        // trail's tail is already mid-dissolve exactly as the shooter saw it.
        let pre = path.points.len();
        for (i, point) in path.points.iter().enumerate() {
            capture_point(
                &mut ribbon,
                *point,
                (pre - i) as f32 * (1.0 / 64.0),
                &mut rng,
            );
        }
        ribbon.consumed = pre;
        let mesh = meshes.add(empty_ribbon_mesh());
        let trail = commands
            .spawn((
                ribbon,
                Mesh3d(mesh),
                MeshMaterial3d(assets.material.clone()),
                // Vertices are authored in world space and the ribbon spans hundreds of meters —
                // rebuilding a tight AABB every frame buys nothing, so skip culling (trails are
                // few, see TRAIL_CAP).
                NoFrustumCulling,
                Transform::IDENTITY,
                NotShadowCaster,
                NotShadowReceiver,
            ))
            .id();
        ring.0.push_back(trail);
        while ring.0.len() > TRAIL_CAP {
            if let Some(old) = ring.0.pop_front() {
                commands.entity(old).try_despawn();
            }
        }
    }
}

/// Capture `pos` as a new trail point if it clears the spacing filter (always captures the first).
fn capture_point(ribbon: &mut TrailRibbon, pos: Vec3, age: f32, rng: &mut ViewRng) {
    if let Some(last) = ribbon.points.back() {
        let gap = pos.distance(last.pos);
        if gap < TRAIL_MIN_SPACING {
            return;
        }
        ribbon.arc += gap;
    }
    // Random lateral direction ⊥ nothing in particular (smoke shear is chaotic) + a constant rise.
    let theta = rng.range(0.0, std::f32::consts::TAU);
    let lateral = Vec3::new(theta.cos(), 0.0, theta.sin()) * rng.range(0.2, TRAIL_DRIFT_LATERAL);
    ribbon.points.push_back(TrailPoint {
        pos,
        age,
        drift: lateral + Vec3::Y * TRAIL_DRIFT_RISE,
        arc: ribbon.arc,
        seed: rng.next_f32(),
    });
    while ribbon.points.len() > TRAIL_MAX_POINTS {
        ribbon.points.pop_front();
    }
}

/// Per-frame trail upkeep: consume new `ShellPath` points while the shell lives, age + expire
/// points, rebuild the camera-facing ribbon mesh, and despawn the trail once its shell is gone and
/// its smoke has fully dissolved.
fn update_trails(
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    shells: Query<(&ShellPath, &Transform)>,
    mut trails: Query<(Entity, &mut TrailRibbon, &Mesh3d)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut rng: ResMut<ViewRng>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let cam = camera.single().map(|c| c.translation()).unwrap_or(Vec3::Y);
    for (entity, mut trail, mesh) in &mut trails {
        // Age first, then expire from the tail.
        for point in trail.points.iter_mut() {
            point.age += dt;
        }
        while trail
            .points
            .front()
            .is_some_and(|p| p.age >= TRAIL_POINT_LIFETIME)
        {
            trail.points.pop_front();
        }

        // Consume the shell's newly recorded points (the recording only appends; on the shell's
        // impact tick it may also have pushed bends we haven't seen — consume before liveness).
        let head = match shells.get(trail.shell) {
            Ok((path, transform)) => {
                let consumed = trail.consumed;
                for point in path.points.iter().skip(consumed) {
                    capture_point(&mut trail, *point, 0.0, &mut rng);
                }
                trail.consumed = path.points.len();
                Some(transform.translation)
            }
            Err(_) => None,
        };

        if head.is_none() && trail.points.len() < 2 {
            // A trail has two lifetime owners — this dissolve-out cleanup and the [`TrailRing`]
            // eviction (which already uses `try_despawn`). If eviction frees a trail first and its
            // slot is recycled before this despawn lands in the shared command flush, a plain
            // `despawn` would warn on the stale id. `try_despawn` makes the second despawn silent.
            commands.entity(entity).try_despawn();
            continue;
        }

        if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
            write_ribbon(&mut mesh, trail.points.make_contiguous(), head, cam);
        }
    }
}

/// An empty triangle-list mesh with the ribbon's attribute set (position/normal/uv/color) — the
/// layout the trail pipeline is specialized against, so the empty and full states are one pipeline.
pub(super) fn empty_ribbon_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    write_ribbon(&mut mesh, &[], None, Vec3::Y);
    mesh
}

/// (Re)build the ribbon geometry into `mesh`: one camera-facing strip through the stored points
/// plus (while the shell lives) a zero-age head vertex pair AT the shell, so the smoke line always
/// reaches the round that is making it.
pub(super) fn write_ribbon(mesh: &mut Mesh, points: &[TrailPoint], head: Option<Vec3>, cam: Vec3) {
    let (positions, uvs, colors) = ribbon_vertices(points, head, cam);
    let quads = (positions.len() / 2).saturating_sub(1);
    let mut indices = Vec::with_capacity(quads * 6);
    for i in 0..quads as u32 {
        let base = i * 2;
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
    }
    let count = positions.len();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; count]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
}

/// The pure strip math (testable headless): per point, the drifted center, the camera-facing side
/// vector, the age-growing width, the arc-anchored V, and the age/seed vertex color the shader
/// erodes by. Returns empty buffers for degenerate inputs (< 2 stations).
fn ribbon_vertices(
    points: &[TrailPoint],
    head: Option<Vec3>,
    cam: Vec3,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 4]>) {
    let station_count = points.len() + usize::from(head.is_some());
    if station_count < 2 {
        return (Vec::new(), Vec::new(), Vec::new());
    }
    // Stations oldest → newest, the head (age 0, at the shell) last.
    let mut centers = Vec::with_capacity(station_count);
    let mut ages = Vec::with_capacity(station_count);
    let mut arcs = Vec::with_capacity(station_count);
    let mut seeds = Vec::with_capacity(station_count);
    for p in points {
        centers.push(p.pos + p.drift * (p.age * p.age));
        ages.push(p.age);
        arcs.push(p.arc);
        seeds.push(p.seed);
    }
    if let Some(head_pos) = head {
        let arc = arcs.last().copied().unwrap_or(0.0)
            + points.last().map_or(0.0, |p| p.pos.distance(head_pos));
        centers.push(head_pos);
        ages.push(0.0);
        arcs.push(arc);
        seeds.push(seeds.last().copied().unwrap_or(0.0));
    }

    // Cut everything closer to the muzzle than TRAIL_START_ARC — that near-barrel smoke is the
    // muzzle puff's job, not the trail's (module doc / TRAIL_START_ARC). Find the first station at
    // or past the start arc; if a station precedes it, splice in a boundary station at EXACTLY the
    // start arc (interpolated between the straddling pair) so the ribbon's near end stays pinned
    // there instead of jumping by a whole station as near-muzzle points age out.
    let n_full = centers.len();
    let start_idx = arcs
        .iter()
        .position(|&a| a >= TRAIL_START_ARC)
        .unwrap_or(n_full);
    let cap = n_full - start_idx + 1;
    let mut e_centers = Vec::with_capacity(cap);
    let mut e_ages = Vec::with_capacity(cap);
    let mut e_arcs = Vec::with_capacity(cap);
    let mut e_seeds = Vec::with_capacity(cap);
    if start_idx < n_full {
        if start_idx > 0 {
            let (a, b) = (start_idx - 1, start_idx);
            let span = arcs[b] - arcs[a];
            let t = if span > 1e-4 {
                (TRAIL_START_ARC - arcs[a]) / span
            } else {
                0.0
            };
            e_centers.push(centers[a].lerp(centers[b], t));
            e_ages.push(ages[a] + (ages[b] - ages[a]) * t);
            e_arcs.push(TRAIL_START_ARC);
            e_seeds.push(seeds[b]);
        }
        e_centers.extend_from_slice(&centers[start_idx..]);
        e_ages.extend_from_slice(&ages[start_idx..]);
        e_arcs.extend_from_slice(&arcs[start_idx..]);
        e_seeds.extend_from_slice(&seeds[start_idx..]);
    }
    let (centers, ages, arcs, seeds) = (e_centers, e_ages, e_arcs, e_seeds);

    let n = centers.len();
    if n < 2 {
        // The whole trail is still inside the start arc (the shell has barely cleared the muzzle):
        // no ribbon yet, the muzzle puff carries the read.
        return (Vec::new(), Vec::new(), Vec::new());
    }
    let mut positions = Vec::with_capacity(n * 2);
    let mut uvs = Vec::with_capacity(n * 2);
    let mut colors = Vec::with_capacity(n * 2);
    // Carried across stations so a degenerate side vector (viewing straight down the trail axis)
    // reuses the last good frame instead of snapping to a world axis — the old `Vec3::Y` fallback
    // flipped the ribbon's down-axis frame-to-frame. `Vec3::Y` seeds only the first station.
    let mut last_side = Vec3::Y;
    for i in 0..n {
        let dir = if i == 0 {
            centers[1] - centers[0]
        } else if i == n - 1 {
            centers[n - 1] - centers[n - 2]
        } else {
            centers[i + 1] - centers[i - 1]
        };
        let side = dir
            .cross(cam - centers[i])
            .try_normalize()
            .unwrap_or(last_side);
        last_side = side;
        // Arc-anchored fade in from the start arc (stage 1a): 0 at TRAIL_START_ARC (the near end,
        // where geometry now begins), easing to 1 over the next HEAD_FADE_ARC metres.
        let fade = smoothstep(TRAIL_START_ARC, TRAIL_START_ARC + HEAD_FADE_ARC, arcs[i]);
        // Width taper from the near end (stage 1b): the age-grown half-width is pinched to ~nothing
        // at the start arc (no 0.25 floor — the near end is an honest wisp point), back to full past
        // the fade. A lens/taper, not a tube with a hard cap.
        let half = 0.5 * (TRAIL_WIDTH_BIRTH + ages[i] * TRAIL_WIDTH_GROWTH) * fade;
        // color.a is the head fade alone (the shader multiplies alpha by it). A view-parallel dim
        // once lived here (stage 1c) but was removed: from any behind-the-gun camera, distance itself
        // aligns receding segments with the view axis (a 100 m segment is <2° off it), so the dim
        // erased the whole far trail no matter the threshold. The cost is the honest flat "ribbon"
        // tell when a trail is seen edge-on; the real fix is a tube cross-section, not an alpha trick.
        let alpha = fade;
        let age_frac = (ages[i] / TRAIL_POINT_LIFETIME).clamp(0.0, 1.0);
        for (edge, u) in [(-1.0, 0.0), (1.0, 1.0)] {
            let p = centers[i] + side * (half * edge);
            positions.push([p.x, p.y, p.z]);
            uvs.push([u, arcs[i]]);
            colors.push([age_frac, seeds[i], 0.0, alpha]);
        }
    }
    (positions, uvs, colors)
}

/// A tiny but real ribbon mesh for the startup pipeline prewarm: same attribute layout and enough
/// geometry that the trail pipeline compiles against exactly what live trails will draw.
pub(super) fn prewarm_ribbon_mesh() -> Mesh {
    let mut mesh = empty_ribbon_mesh();
    let points = [
        TrailPoint {
            pos: Vec3::ZERO,
            age: 1.0,
            drift: Vec3::Y * 0.3,
            arc: 0.0,
            seed: 0.3,
        },
        TrailPoint {
            pos: Vec3::X * TRAIL_MIN_SPACING,
            age: 0.5,
            drift: Vec3::Y * 0.3,
            arc: TRAIL_MIN_SPACING,
            seed: 0.7,
        },
    ];
    write_ribbon(
        &mut mesh,
        &points,
        Some(Vec3::X * TRAIL_MIN_SPACING * 2.0),
        Vec3::Y * 50.0,
    );
    mesh
}

/// A 128×128 tileable two-octave value-noise texture (R8, repeat sampler) — the trail's breakup
/// signal, generated at startup instead of shipping another asset. Deterministic (hash lattice),
/// but purely cosmetic either way.
fn noise_texture(images: &mut Assets<Image>) -> Handle<Image> {
    use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
    const SIZE: usize = 128;
    // Integer lattice hash → [0,1) (splitmix-flavored, wrap-friendly): decorrelate the packed
    // coords with the golden-ratio increment, then the shared two-round bit mix (no trailing
    // xorshift here — see `super::mix64`), taking the top 24 bits.
    fn lattice(x: u32, y: u32, octave: u32) -> f32 {
        let z = (x as u64) ^ ((y as u64) << 16) ^ ((octave as u64) << 40);
        let z = super::mix64(z.wrapping_add(0x9E37_79B9_7F4A_7C15));
        ((z >> 40) as f32) / (1u64 << 24) as f32
    }
    fn smooth(t: f32) -> f32 {
        t * t * (3.0 - 2.0 * t)
    }
    // Periodic bilinear value noise at `cells` lattice cells across the texture.
    fn value_noise(px: usize, py: usize, cells: u32, octave: u32) -> f32 {
        let fx = px as f32 / SIZE as f32 * cells as f32;
        let fy = py as f32 / SIZE as f32 * cells as f32;
        let (x0, y0) = (fx.floor() as u32 % cells, fy.floor() as u32 % cells);
        let (x1, y1) = ((x0 + 1) % cells, (y0 + 1) % cells);
        let (tx, ty) = (smooth(fx.fract()), smooth(fy.fract()));
        let a = lattice(x0, y0, octave);
        let b = lattice(x1, y0, octave);
        let c = lattice(x0, y1, octave);
        let d = lattice(x1, y1, octave);
        (a + (b - a) * tx) + ((c + (d - c) * tx) - (a + (b - a) * tx)) * ty
    }
    let mut data = Vec::with_capacity(SIZE * SIZE);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let v = 0.65 * value_noise(x, y, 8, 0) + 0.35 * value_noise(x, y, 21, 1);
            data.push((v.clamp(0.0, 1.0) * 255.0) as u8);
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SIZE as u32,
            height: SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    // The shader scrolls indefinitely along the trail: the sampler must REPEAT (the default
    // clamps).
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..ImageSamplerDescriptor::linear()
    });
    images.add(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(pos: Vec3, age: f32, arc: f32) -> TrailPoint {
        TrailPoint {
            pos,
            age,
            drift: Vec3::Y * 0.3,
            arc,
            seed: 0.5,
        }
    }

    /// The strip math after the start-arc cut: 2 vertices per station, 6 indices per quad, NO
    /// geometry inside TRAIL_START_ARC (the near-muzzle station is dropped and replaced by an
    /// interpolated boundary pinned at exactly the start arc), the near end a width-and-alpha wisp
    /// easing to full over HEAD_FADE_ARC, and the head (age 0) narrowed by the age term.
    #[test]
    fn ribbon_vertices_shape_and_taper() {
        // arc 5 is inside the start arc (dropped); the others straddle/clear it.
        let points = vec![
            point(Vec3::X * 5.0, 1.0, 5.0),
            point(Vec3::X * 20.0, 0.8, 20.0),
            point(Vec3::X * 40.0, 0.4, 40.0),
        ];
        let head = Some(Vec3::X * 50.0); // head arc = 40 + 10 = 50
        let cam = Vec3::new(20.0, 20.0, 40.0);
        let (positions, uvs, colors) = ribbon_vertices(&points, head, cam);
        // Emitted: boundary(arc 10) + arc 20 + arc 40 + head(arc 50) = 4 stations. The arc-5
        // station is gone, spliced into the boundary.
        assert_eq!(positions.len(), 8, "boundary + 2 kept points + head");
        // No emitted vertex lives inside the start arc.
        for uv in &uvs {
            assert!(
                uv[1] >= TRAIL_START_ARC,
                "no geometry emitted inside TRAIL_START_ARC (got arc {})",
                uv[1]
            );
        }
        let width_at = |i: usize| {
            let a = Vec3::from(positions[i * 2]);
            let b = Vec3::from(positions[i * 2 + 1]);
            a.distance(b)
        };
        // Near end is a pinched-to-nothing wisp (fade = 0 at the start arc), widening outward.
        assert_eq!(width_at(0), 0.0, "the near end is pinched to zero width");
        assert!(
            width_at(1) > width_at(0),
            "width fades in past the start arc"
        );
        assert!(width_at(2) > width_at(1), "still widening across the fade");
        // The head (age 0) is narrower than the same-full-fade older station just behind it.
        assert!(
            width_at(2) > width_at(3),
            "the head (age 0) narrows by the age term"
        );
        // V rides the capture-time arc: it now STARTS at the boundary arc, monotonic to the head.
        assert_eq!(
            uvs[0][1], TRAIL_START_ARC,
            "near end pinned at the start arc"
        );
        assert_eq!(uvs[2][1], 20.0);
        assert!(
            uvs[6][1] > uvs[4][1],
            "head arc extends past the last point"
        );
        // The age lane feeds the shader's erosion: the near (older) end eroded, head fresh.
        assert!(colors[0][0] > 0.4 && colors[6][0] == 0.0);
        // The alpha lane is the arc fade ALONE (no view-parallel dim): fully transparent at the near
        // end (the start arc), full alpha past TRAIL_START_ARC + HEAD_FADE_ARC (arc 40 clears 30 m)
        // regardless of view angle.
        assert_eq!(colors[0][3], 0.0, "near-end fade is fully in");
        assert_eq!(
            colors[4][3], 1.0,
            "past the fade window the trail is at full alpha (fade only, never view-dimmed)"
        );

        // A trail still entirely inside the start arc builds NOTHING (the shell just cleared the
        // muzzle — the puff carries it).
        let inside = vec![
            point(Vec3::X * 2.0, 0.5, 2.0),
            point(Vec3::X * 6.0, 0.3, 6.0),
        ];
        let (p, _, _) = ribbon_vertices(&inside, Some(Vec3::X * 8.0), cam);
        assert!(
            p.is_empty(),
            "no geometry until the trail clears the start arc"
        );

        // Degenerate: one station (or none) builds nothing.
        let (p, _, _) = ribbon_vertices(&points[..1], None, cam);
        assert!(p.is_empty());
        let (p, _, _) = ribbon_vertices(&[], None, cam);
        assert!(p.is_empty());
    }

    /// Capture discipline: the spacing filter drops close points, the arc accumulates only over
    /// captured ones, and the hard cap evicts oldest-first.
    #[test]
    fn capture_respects_spacing_and_cap() {
        let mut rng = ViewRng::seeded(3);
        let mut ribbon = TrailRibbon {
            shell: Entity::PLACEHOLDER,
            points: VecDeque::new(),
            consumed: 0,
            arc: 0.0,
        };
        capture_point(&mut ribbon, Vec3::ZERO, 0.0, &mut rng);
        // Too close: filtered.
        capture_point(
            &mut ribbon,
            Vec3::X * (TRAIL_MIN_SPACING * 0.5),
            0.0,
            &mut rng,
        );
        assert_eq!(ribbon.points.len(), 1, "sub-spacing point must be dropped");
        // Far enough: captured, arc advanced by the true gap.
        capture_point(
            &mut ribbon,
            Vec3::X * (TRAIL_MIN_SPACING * 1.5),
            0.0,
            &mut rng,
        );
        assert_eq!(ribbon.points.len(), 2);
        assert_eq!(ribbon.points[1].arc, TRAIL_MIN_SPACING * 1.5);

        // The cap holds under a long flight.
        for i in 0..TRAIL_MAX_POINTS + 20 {
            capture_point(
                &mut ribbon,
                Vec3::X * (i as f32 + 2.0) * TRAIL_MIN_SPACING,
                0.0,
                &mut rng,
            );
        }
        assert_eq!(ribbon.points.len(), TRAIL_MAX_POINTS);
    }

    /// The live wiring on real ECS systems: an 88-signature shell (ShellPath + WorldAssetRoot)
    /// grows a ribbon whose mesh has geometry; an MG round (no scene root) never does; and once the
    /// shell despawns the ribbon outlives it only until its smoke fully expires.
    #[test]
    fn trails_attach_follow_and_outlive_the_shell() {
        let mut app = App::new();
        app.init_resource::<TrailRing>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<VfxTrailMaterial>>()
            .init_resource::<Time>()
            .insert_resource(ViewRng::seeded(9))
            .add_systems(Update, (attach_trails, update_trails).chain());
        let material = app
            .world_mut()
            .resource_mut::<Assets<VfxTrailMaterial>>()
            .add(VfxTrailMaterial {
                params: TrailParams {
                    shape: Vec4::ONE,
                    glow: Vec4::ZERO,
                },
                noise: Handle::default(),
                lut: Handle::default(),
            });
        app.insert_resource(TrailAssets { material });

        // An 88-signature shell mid-flight, and an MG round (ShellPath only).
        let shell = app
            .world_mut()
            .spawn((
                ShellPath {
                    points: vec![Vec3::ZERO, Vec3::X * 20.0, Vec3::X * 40.0],
                },
                WorldAssetRoot::default(),
                Transform::from_translation(Vec3::X * 50.0),
            ))
            .id();
        app.world_mut().spawn((
            ShellPath {
                points: vec![Vec3::ZERO, Vec3::X * 20.0],
            },
            Transform::from_translation(Vec3::X * 25.0),
        ));
        app.update();

        let world = app.world_mut();
        let trails: Vec<Entity> = world
            .query_filtered::<Entity, With<TrailRibbon>>()
            .iter(world)
            .collect();
        assert_eq!(
            trails.len(),
            1,
            "exactly the 88 grows a trail, never the MG"
        );
        let mesh_handle = world
            .get::<Mesh3d>(trails[0])
            .expect("ribbon mesh")
            .0
            .clone();
        let vertex_count = |world: &World| {
            world
                .resource::<Assets<Mesh>>()
                .get(&mesh_handle)
                .expect("mesh asset")
                .count_vertices()
        };
        assert!(
            vertex_count(app.world()) >= 6,
            "captured path + live head must build strip geometry"
        );

        // Shell flies on: new path points get consumed.
        app.world_mut()
            .get_mut::<ShellPath>(shell)
            .expect("shell alive")
            .points
            .push(Vec3::X * 60.0);
        app.update();
        let consumed = app
            .world_mut()
            .query::<&TrailRibbon>()
            .single(app.world())
            .expect("one trail")
            .consumed;
        assert_eq!(consumed, 4, "the ribbon consumes the shell's recording");

        // Impact: the shell despawns; the ribbon must survive it...
        app.world_mut().despawn(shell);
        app.update();
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<TrailRibbon>>()
                .iter(app.world())
                .count(),
            1,
            "the trail outlives its shell"
        );
        // ...until every point ages out, then clean itself up.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(
                TRAIL_POINT_LIFETIME + 0.1,
            ));
        app.update();
        app.update();
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<TrailRibbon>>()
                .iter(app.world())
                .count(),
            0,
            "a fully dissolved orphan trail despawns"
        );
    }
}
