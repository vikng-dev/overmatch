//! Main-gun smoke ribbon derived from [`ShellPath`].
//!
//! Invariant (ADR-0014): view state never extends a simulation path. Disconnected path segments
//! receive no bridge triangles.

use std::collections::VecDeque;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::Image;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, MeshVertexAttribute, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use serde_json::json;

use crate::ballistics::{PenetrationMarks, ShellPath, ShellVisual, Shot};
use crate::view::PlayerView;
use crate::{ShotClock, ShotId};

use super::ViewRng;
use super::billboard::{gradient_lut, smoothstep};

/// Minimum spacing (m) between captured trail points.
const TRAIL_MIN_SPACING: f32 = 14.0;
/// A trail point's lifetime (s) — the tail fully erodes by then (the shader's age axis).
const TRAIL_POINT_LIFETIME: f32 = 2.2;
/// Hard per-trail point cap.
const TRAIL_MAX_POINTS: usize = 128;
/// Ribbon width (m): at birth, plus growth per second of point age (smoke expands as it ages).
const TRAIL_WIDTH_BIRTH: f32 = 0.35;
const TRAIL_WIDTH_GROWTH: f32 = 1.3;
/// Per-point lateral drift and rise.
const TRAIL_DRIFT_LATERAL: f32 = 0.4;
const TRAIL_DRIFT_RISE: f32 = 0.35;
/// Live-trail ring cap.
const TRAIL_CAP: usize = 8;
/// Arc length from the muzzle before ribbon geometry begins. The boundary is interpolated so expiry
/// cannot move it by one captured station.
const TRAIL_START_ARC: f32 = 10.0;
/// Taper length after [`TRAIL_START_ARC`].
const HEAD_FADE_ARC: f32 = 20.0;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TrailRing>()
        .add_plugins(MaterialPlugin::<VfxTrailMaterial>::default())
        .add_systems(Startup, setup_trail_assets)
        .add_observer(attach_trail)
        .add_observer(flush_removed_shell_path)
        // Invariant: rebuild after transform propagation.
        .add_systems(PostUpdate, update_trails.after(TransformSystems::Propagate));
}

/// Mount the minimum real trail pipeline the loss-injected harness needs. This deliberately omits
/// the render-world material plugin: the harness proves main-world station admission and ribbon
/// construction, while the production plugin owns GPU extraction and specialization.
#[cfg(test)]
pub(super) fn mount_loss_harness(app: &mut App) {
    app.init_resource::<TrailRing>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<VfxTrailMaterial>>()
        .init_resource::<ViewRng>()
        .add_message::<TrailStationMeshEvidence>()
        .add_observer(attach_trail)
        .add_observer(flush_removed_shell_path)
        .add_systems(PostUpdate, update_trails.in_set(TrailHarnessSet));
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
}

/// The loss harness collects its proof after this real trail-consumption set has run.
#[cfg(test)]
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TrailHarnessSet;

/// Test-only main-world ribbon evidence. It is emitted only after the spacing filter admitted a
/// post-anchor station and [`write_ribbon`] produced non-empty mesh vertices and indices. It does
/// not claim render-world extraction or GPU visibility.
#[cfg(test)]
#[derive(Message, Debug, Clone, Copy)]
pub(crate) struct TrailStationMeshEvidence {
    pub(crate) shot: ShotId,
    pub(crate) sequence: u32,
    pub(crate) vertices: usize,
    pub(crate) indices: usize,
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

/// Marker on a shell whose trail has been attached (so [`attach_trail`] runs once per shell).
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
    /// Index of the raw [`ShellPath`] point this station consumed — instrumentation can prove a
    /// rendered station lies strictly after an authoritative bounce re-anchor.
    pub source_index: usize,
    /// Disconnected [`ShellPath`] segment this station belongs to. Adjacent stations from different
    /// segments deliberately receive no ribbon indices between them.
    pub segment: u32,
}

/// A sanctioned bounce seen in `PenetrationMarks` but not yet represented by a captured ribbon
/// station strictly after its re-anchor.
#[derive(Clone, Copy)]
struct PendingTrailBounce {
    sequence: u32,
    path_index: usize,
    segment: u32,
}

/// A live smoke ribbon following (then outliving) one shell.
#[derive(Component)]
pub(crate) struct TrailRibbon {
    shell: Entity,
    shot: Option<ShotId>,
    points: VecDeque<TrailPoint>,
    /// `ShellPath::points` consumed so far (the recording only appends).
    consumed: usize,
    /// Running arc length at the last captured point.
    arc: f32,
    /// Ricochet marks already converted into pending/confirmed trail-boundary observations.
    observed_ricochets: usize,
    pending_bounces: VecDeque<PendingTrailBounce>,
    /// Bounces whose spacing-filtered station was consumed and await main-world ribbon evidence.
    #[cfg(test)]
    mesh_evidence_pending: VecDeque<PendingTrailBounce>,
}

/// Live trails, oldest first — the refire leak bound (see [`TRAIL_CAP`]).
#[derive(Resource, Default)]
struct TrailRing(VecDeque<Entity>);

/// Give every new main-gun shell a trail ribbon. The gate is `ShellVisual` — the marker
/// `ballistics::view` puts on a shell-class round, and the same one the ember keys on.
///
/// It OBSERVES the dressing rather than polling for it: the ribbon then opens on the round's real
/// birth tick in whichever schedule spawned it, so a shell whose whole flight resolves between two
/// rendered frames still has its path captured.
fn attach_trail(
    add: On<Add, ShellVisual>,
    shells: Query<(&ShellPath, Option<&Shot>), Without<TrailedShell>>,
    mut meshes: ResMut<Assets<Mesh>>,
    assets: Res<TrailAssets>,
    mut ring: ResMut<TrailRing>,
    mut rng: ResMut<ViewRng>,
    mut commands: Commands,
) {
    let shell = add.entity;
    let Ok((path, shot)) = shells.get(shell) else {
        return;
    };
    commands.entity(shell).insert(TrailedShell);
    let mut ribbon = TrailRibbon {
        shell,
        shot: shot.map(|shot| shot.0),
        points: VecDeque::new(),
        consumed: 0,
        arc: 0.0,
        observed_ricochets: 0,
        pending_bounces: VecDeque::new(),
        #[cfg(test)]
        mesh_evidence_pending: VecDeque::new(),
    };
    // Catch-up points receive elapsed ages so the ribbon starts at its current flight state.
    let pre = path.points.len();
    for (i, point) in path.points.iter().enumerate() {
        capture_point(
            &mut ribbon,
            *point,
            (pre - i) as f32 * (1.0 / 64.0),
            i,
            path_segment(path, i),
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
            // rebuilding a tight AABB every frame buys nothing, so skip culling (trails are few, see
            // TRAIL_CAP).
            NoFrustumCulling,
            Transform::IDENTITY,
            NotShadowCaster,
            NotShadowReceiver,
        ))
        .id();
    crate::push_capped_entity(&mut commands, &mut ring.0, trail, TRAIL_CAP);
}

/// Capture `pos` as a new trail point if it clears the spacing filter (always captures the first).
fn capture_point(
    ribbon: &mut TrailRibbon,
    pos: Vec3,
    age: f32,
    source_index: usize,
    segment: u32,
    rng: &mut ViewRng,
) -> bool {
    if let Some(last) = ribbon.points.back()
        && last.segment == segment
    {
        let gap = pos.distance(last.pos);
        if gap < TRAIL_MIN_SPACING {
            return false;
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
        source_index,
        segment,
    });
    while ribbon.points.len() > TRAIL_MAX_POINTS {
        ribbon.points.pop_front();
    }
    true
}

/// Segment ordinal for one append-only path index. `segment_starts` stores every disconnected start
/// after index zero in sorted append order.
fn path_segment(path: &ShellPath, index: usize) -> u32 {
    path.segment_starts.partition_point(|&start| start <= index) as u32
}

/// Consume every raw path point and bounce mark not yet seen by this ribbon, then record which
/// sanctioned bounces have crossed the actual spacing-filtered renderer boundary.
fn consume_recorded_path(
    trail: &mut TrailRibbon,
    path: &ShellPath,
    marks: &PenetrationMarks,
    rng: &mut ViewRng,
    now: u32,
    shot_trace: &mut Option<ResMut<'_, crate::shot_trace::ShotTrace>>,
) {
    // `rposition` selects the authoritative re-anchor occurrence when a local candidate happened to
    // share the same point earlier in the path.
    for (sequence, origin) in marks
        .ricochets
        .iter()
        .enumerate()
        .skip(trail.observed_ricochets)
    {
        if let Some(path_index) = path
            .points
            .iter()
            .rposition(|point| point.distance_squared(*origin) < 1.0e-6)
        {
            trail.pending_bounces.push_back(PendingTrailBounce {
                sequence: sequence as u32,
                path_index,
                segment: path_segment(path, path_index),
            });
        } else if let Some(shot) = trail.shot {
            crate::shot_trace::record(
                shot_trace,
                "trail",
                now,
                shot,
                || json!({ "res": "bounce_anchor_missing", "seq": sequence }),
            );
        }
    }
    trail.observed_ricochets = marks.ricochets.len();

    let consumed = trail.consumed.min(path.points.len());
    for (index, point) in path.points.iter().enumerate().skip(consumed) {
        capture_point(trail, *point, 0.0, index, path_segment(path, index), rng);
    }
    trail.consumed = path.points.len();

    // This is the station-admission boundary: a bounce is consumed only after spacing admitted a
    // station strictly after its authoritative re-anchor, on that disconnected segment. The test
    // harness emits its main-world mesh evidence after the subsequent `write_ribbon` rebuild.
    let mut still_pending = VecDeque::new();
    while let Some(pending) = trail.pending_bounces.pop_front() {
        let rendered = trail.points.iter().any(|point| {
            point.segment == pending.segment && point.source_index > pending.path_index
        });
        if rendered {
            if let Some(shot) = trail.shot {
                #[cfg(test)]
                trail.mesh_evidence_pending.push_back(pending);
                let stations = trail.points.len();
                crate::shot_trace::record(shot_trace, "trail", now, shot, || {
                    json!({
                        "res": "post_bounce_consumed",
                        "seq": pending.sequence,
                        "pi": pending.path_index,
                        "stations": stations,
                    })
                });
            }
        } else {
            still_pending.push_back(pending);
        }
    }
    trail.pending_bounces = still_pending;
}

/// Close any bounce obligations still lacking a rendered post-bounce station when the path ends.
fn finish_pending_bounces(
    trail: &mut TrailRibbon,
    now: u32,
    shot_trace: &mut Option<ResMut<'_, crate::shot_trace::ShotTrace>>,
) {
    if let Some(shot) = trail.shot {
        while let Some(pending) = trail.pending_bounces.pop_front() {
            crate::shot_trace::record(shot_trace, "trail", now, shot, || {
                json!({
                    "res": "post_bounce_unrendered",
                    "seq": pending.sequence,
                    "pi": pending.path_index,
                })
            });
        }
    } else {
        trail.pending_bounces.clear();
    }
}

/// Bevy 0.19's `On<Remove, T>` runs BEFORE `T` is removed, so a despawning shell's final fixed-tick
/// path is still queryable here. Flush it into the already-detached ribbon synchronously; otherwise a
/// bounce and terminal between render frames would lose the last segment before `update_trails` saw it.
fn flush_removed_shell_path(
    remove: On<Remove, ShellPath>,
    shells: Query<(&ShellPath, &PenetrationMarks, Option<&Shot>), With<ShellVisual>>,
    mut trails: Query<&mut TrailRibbon>,
    mut rng: ResMut<ViewRng>,
    present: Option<Res<ShotClock>>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    let Ok((path, marks, shot)) = shells.get(remove.entity) else {
        return; // MG/no-scene shell: it never owns a smoke ribbon.
    };
    let now = present.as_deref().map_or(0, |present| present.0);
    let Some(mut trail) = trails.iter_mut().find(|trail| trail.shell == remove.entity) else {
        if let Some(shot) = shot {
            crate::shot_trace::record(&mut shot_trace, "trail", now, shot.0, || {
                json!({
                    "res": "ribbon_missing_at_end",
                    "points": path.points.len(),
                    "bounces": marks.ricochets.len(),
                })
            });
        }
        return;
    };
    consume_recorded_path(&mut trail, path, marks, &mut rng, now, &mut shot_trace);
    finish_pending_bounces(&mut trail, now, &mut shot_trace);
}

/// Per-frame trail upkeep: consume new `ShellPath` points while the shell lives, age + expire
/// points, rebuild the camera-facing ribbon mesh, and despawn the trail once its shell is gone and
/// its smoke has fully dissolved.
#[expect(
    clippy::too_many_arguments,
    reason = "per-frame trail upkeep spanning the clock, camera, shells, trails, meshes, rng, net clock, trace, evidence and the station scratch"
)]
fn update_trails(
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<PlayerView>>,
    shells: Query<(&ShellPath, &Transform, &PenetrationMarks)>,
    mut trails: Query<(Entity, &mut TrailRibbon, &Mesh3d)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut rng: ResMut<ViewRng>,
    present: Option<Res<ShotClock>>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
    #[cfg(test)] mut mesh_evidence: MessageWriter<TrailStationMeshEvidence>,
    mut stations: Local<Vec<Station>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let now = present.as_deref().map_or(0, |present| present.0);
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
        // impact tick the removal observer flushes bends synchronously before liveness disappears).
        let head = match shells.get(trail.shell) {
            Ok((path, transform, marks)) => {
                consume_recorded_path(&mut trail, path, marks, &mut rng, now, &mut shot_trace);
                Some(transform.translation)
            }
            Err(_) => None,
        };
        if head.is_none() {
            finish_pending_bounces(&mut trail, now, &mut shot_trace);
        }

        if head.is_none() && trail.points.len() < 2 {
            // A trail has two lifetime owners — this dissolve-out cleanup and the [`TrailRing`]
            // eviction (which already uses `try_despawn`). If eviction frees a trail first and its
            // slot is recycled before this despawn lands in the shared command flush, a plain
            // `despawn` would warn on the stale id. `try_despawn` makes the second despawn silent.
            commands.entity(entity).try_despawn();
            continue;
        }

        if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
            write_ribbon(
                &mut mesh,
                trail.points.make_contiguous(),
                head,
                cam,
                &mut stations,
            );
            #[cfg(test)]
            emit_mesh_evidence(&mut trail, &mesh, &mut mesh_evidence);
        }
    }
}

/// Publish test evidence only after the real CPU ribbon rebuild wrote drawable main-world geometry.
#[cfg(test)]
fn emit_mesh_evidence(
    trail: &mut TrailRibbon,
    mesh: &Mesh,
    evidence: &mut MessageWriter<TrailStationMeshEvidence>,
) {
    let vertices = mesh.count_vertices();
    let indices = mesh.indices().map_or(0, Indices::len);
    if vertices == 0 || indices == 0 {
        return;
    }
    let Some(shot) = trail.shot else {
        trail.mesh_evidence_pending.clear();
        return;
    };
    while let Some(bounce) = trail.mesh_evidence_pending.pop_front() {
        evidence.write(TrailStationMeshEvidence {
            shot,
            sequence: bounce.sequence,
            vertices,
            indices,
        });
    }
}

/// An empty triangle-list mesh with the ribbon's attribute set (position/normal/uv/color) — the
/// layout the trail pipeline is specialized against, so the empty and full states are one pipeline.
pub(super) fn empty_ribbon_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    write_ribbon(&mut mesh, &[], None, Vec3::Y, &mut Vec::new());
    mesh
}

/// One ribbon station: the drifted center and what its two vertices carry.
#[derive(Clone, Copy)]
pub(super) struct Station {
    center: Vec3,
    age: f32,
    arc: f32,
    seed: f32,
    segment: u32,
}

/// (Re)build the ribbon geometry into `mesh`: camera-facing strips through each disconnected stored
/// segment, plus (while the shell lives) a zero-age head vertex pair AT the shell, so the newest strip
/// reaches the round that is making it. All strips remain one mesh/draw.
///
/// Allocation-free once warm: the mesh's own attribute and index vectors are refilled in place and
/// `stations` is the caller's scratch, so a rebuild allocates only while its ribbon, or the
/// scratch, grows past its largest earlier size. The positions are rewritten whole every frame
/// because the strip faces `cam`.
pub(super) fn write_ribbon(
    mesh: &mut Mesh,
    points: &[TrailPoint],
    head: Option<Vec3>,
    cam: Vec3,
    stations: &mut Vec<Station>,
) {
    ribbon_stations(points, head, stations);
    let mut positions = take_attribute(mesh, Mesh::ATTRIBUTE_POSITION, |values| match values {
        VertexAttributeValues::Float32x3(values) => Some(values),
        _ => None,
    });
    let mut normals = take_attribute(mesh, Mesh::ATTRIBUTE_NORMAL, |values| match values {
        VertexAttributeValues::Float32x3(values) => Some(values),
        _ => None,
    });
    let mut uvs = take_attribute(mesh, Mesh::ATTRIBUTE_UV_0, |values| match values {
        VertexAttributeValues::Float32x2(values) => Some(values),
        _ => None,
    });
    let mut colors = take_attribute(mesh, Mesh::ATTRIBUTE_COLOR, |values| match values {
        VertexAttributeValues::Float32x4(values) => Some(values),
        _ => None,
    });
    let mut indices = match mesh.indices_mut() {
        Some(Indices::U32(indices)) => std::mem::take(indices),
        _ => Vec::new(),
    };
    ribbon_vertices(stations, cam, &mut positions, &mut uvs, &mut colors);
    ribbon_indices(stations, &mut indices);
    normals.resize(positions.len(), [0.0, 1.0, 0.0]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
}

/// `attribute`'s vector moved out of `mesh` with its capacity, emptied for a refill — or a new one
/// on the first build. `values` names the variant the attribute's format stores.
fn take_attribute<T>(
    mesh: &mut Mesh,
    attribute: MeshVertexAttribute,
    values: fn(&mut VertexAttributeValues) -> Option<&mut Vec<T>>,
) -> Vec<T> {
    let mut taken = mesh
        .attribute_mut(attribute.id)
        .and_then(values)
        .map(std::mem::take)
        .unwrap_or_default();
    taken.clear();
    taken
}

/// Triangle indices for the station strip into `indices`, deliberately omitting the quad between
/// disconnected path segments. Each station owns two adjacent vertices.
fn ribbon_indices(stations: &[Station], indices: &mut Vec<u32>) {
    indices.clear();
    for (i, adjacent) in stations.windows(2).enumerate() {
        if adjacent[0].segment != adjacent[1].segment {
            continue;
        }
        let base = i as u32 * 2;
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
    }
}

/// The stations a ribbon draws, oldest → newest, into `out`: each point's drifted center, then the
/// live head (age 0, at the shell). Empty for degenerate inputs (< 2 stations).
fn ribbon_stations(points: &[TrailPoint], head: Option<Vec3>, out: &mut Vec<Station>) {
    out.clear();
    if points.len() + usize::from(head.is_some()) < 2 {
        return;
    }
    out.extend(points.iter().map(|p| Station {
        center: p.pos + p.drift * (p.age * p.age),
        age: p.age,
        arc: p.arc,
        seed: p.seed,
        segment: p.segment,
    }));
    if let Some(head_pos) = head {
        let newest = out.last().copied();
        out.push(Station {
            center: head_pos,
            age: 0.0,
            arc: newest.map_or(0.0, |s| s.arc)
                + points.last().map_or(0.0, |p| p.pos.distance(head_pos)),
            seed: newest.map_or(0.0, |s| s.seed),
            segment: newest.map_or(0, |s| s.segment),
        });
    }

    // Cut everything closer to the muzzle than TRAIL_START_ARC — that near-barrel smoke is the
    // muzzle puff's job, not the trail's (module doc / TRAIL_START_ARC). Find the first station at
    // or past the start arc; if a station precedes it, splice in a boundary station at EXACTLY the
    // start arc (interpolated between the straddling pair) so the ribbon's near end stays pinned
    // there instead of jumping by a whole station as near-muzzle points age out.
    let start = out
        .iter()
        .position(|s| s.arc >= TRAIL_START_ARC)
        .unwrap_or(out.len());
    if start == out.len() {
        out.clear();
    } else if start > 0 && out[start - 1].segment == out[start].segment {
        let (a, b) = (out[start - 1], out[start]);
        let span = b.arc - a.arc;
        let t = if span > 1e-4 {
            (TRAIL_START_ARC - a.arc) / span
        } else {
            0.0
        };
        out[start - 1] = Station {
            center: a.center.lerp(b.center, t),
            age: a.age + (b.age - a.age) * t,
            arc: TRAIL_START_ARC,
            seed: b.seed,
            segment: b.segment,
        };
        out.drain(..start - 1);
    } else {
        out.drain(..start);
    }
    if out.len() < 2 {
        // The whole trail is still inside the start arc (the shell has barely cleared the muzzle):
        // no ribbon yet, the muzzle puff carries the read.
        out.clear();
    }
}

/// The pure strip math (testable headless), appended to the three vertex buffers: per station, the
/// camera-facing side vector, the age-growing width, the arc-anchored V, and the age/seed vertex
/// color the shader erodes by.
fn ribbon_vertices(
    stations: &[Station],
    cam: Vec3,
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    colors: &mut Vec<[f32; 4]>,
) {
    let n = stations.len();
    // Carried across stations so a degenerate side vector (viewing straight down the trail axis)
    // reuses the last good frame instead of snapping to a world axis — the old `Vec3::Y` fallback
    // flipped the ribbon's down-axis frame-to-frame. `Vec3::Y` seeds only the first station.
    let mut last_side = Vec3::Y;
    for i in 0..n {
        let station = stations[i];
        let same_prev = i > 0 && stations[i - 1].segment == station.segment;
        let same_next = i + 1 < n && stations[i + 1].segment == station.segment;
        let dir = match (same_prev, same_next) {
            (true, true) => stations[i + 1].center - stations[i - 1].center,
            (true, false) => station.center - stations[i - 1].center,
            (false, true) => stations[i + 1].center - station.center,
            (false, false) => Vec3::X,
        };
        let side = dir
            .cross(cam - station.center)
            .try_normalize()
            .unwrap_or(last_side);
        last_side = side;
        // Taper geometry from the start arc.
        let fade = smoothstep(
            TRAIL_START_ARC,
            TRAIL_START_ARC + HEAD_FADE_ARC,
            station.arc,
        );
        let half = 0.5 * (TRAIL_WIDTH_BIRTH + station.age * TRAIL_WIDTH_GROWTH) * fade;
        // The shader multiplies alpha by this near-end taper.
        let alpha = fade;
        let age_frac = (station.age / TRAIL_POINT_LIFETIME).clamp(0.0, 1.0);
        for (edge, u) in [(-1.0, 0.0), (1.0, 1.0)] {
            let p = station.center + side * (half * edge);
            positions.push([p.x, p.y, p.z]);
            uvs.push([u, station.arc]);
            colors.push([age_frac, station.seed, 0.0, alpha]);
        }
    }
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
            source_index: 0,
            segment: 0,
        },
        TrailPoint {
            pos: Vec3::X * TRAIL_MIN_SPACING,
            age: 0.5,
            drift: Vec3::Y * 0.3,
            arc: TRAIL_MIN_SPACING,
            seed: 0.7,
            source_index: 1,
            segment: 0,
        },
    ];
    write_ribbon(
        &mut mesh,
        &points,
        Some(Vec3::X * TRAIL_MIN_SPACING * 2.0),
        Vec3::Y * 50.0,
        &mut Vec::new(),
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
            source_index: arc as usize,
            segment: 0,
        }
    }

    #[test]
    fn disconnected_reseed_has_no_bridge_quad() {
        let mut points = vec![
            point(Vec3::X * 20.0, 0.8, 20.0),
            point(Vec3::X * 40.0, 0.7, 40.0),
            point(Vec3::new(140.0, 0.0, 0.0), 0.2, 40.0),
            point(Vec3::new(160.0, 0.0, 0.0), 0.1, 60.0),
        ];
        points[2].segment = 1;
        points[3].segment = 1;
        let mut stations = Vec::new();
        ribbon_stations(&points, None, &mut stations);
        let segments: Vec<u32> = stations.iter().map(|station| station.segment).collect();
        assert_eq!(segments, vec![0, 0, 1, 1]);
        let mut indices = Vec::new();
        ribbon_indices(&stations, &mut indices);
        assert_eq!(
            indices.len(),
            12,
            "two two-station segments produce two quads, never a correction bridge"
        );
        assert_eq!(
            indices,
            vec![0, 1, 2, 1, 3, 2, 4, 5, 6, 5, 7, 6],
            "no triangle connects the final pre-correction station to the re-seed"
        );
    }

    /// A warm ribbon rebuild allocates nothing: the mesh's attribute and index vectors and the
    /// station scratch are refilled in place. Every live trail is rebuilt at render rate, so a
    /// rebuild that collected fresh vectors would allocate a dozen-odd of them per trail per frame.
    #[test]
    fn a_warm_ribbon_rebuild_allocates_nothing() {
        let points: Vec<TrailPoint> = (0..40)
            .map(|i| {
                let arc = i as f32 * TRAIL_MIN_SPACING;
                point(Vec3::X * arc, 0.05 * i as f32, arc)
            })
            .collect();
        let head = Some(Vec3::X * 41.0 * TRAIL_MIN_SPACING);
        let mut mesh = empty_ribbon_mesh();
        let mut stations = Vec::new();
        // Warm-up: the longest build sizes every buffer.
        write_ribbon(
            &mut mesh,
            &points,
            head,
            Vec3::new(0.0, 50.0, 50.0),
            &mut stations,
        );

        // Later frames: the camera moves and the tail expires, so every vertex is rewritten.
        let allocations = crate::tests::alloc_count::allocations_in(|| {
            for frame in 1..=8 {
                let cam = Vec3::new(frame as f32, 50.0, 50.0 - frame as f32);
                write_ribbon(&mut mesh, &points[frame..], head, cam, &mut stations);
            }
        });
        assert_eq!(allocations, 0, "a warm ribbon rebuild must not allocate");
        assert!(
            !stations.is_empty() && mesh.count_vertices() == 2 * stations.len(),
            "the rebuild still writes the whole strip"
        );
    }

    /// Capture discipline: the spacing filter drops close points, the arc accumulates only over
    /// captured ones, and the hard cap evicts oldest-first.
    #[test]
    fn capture_respects_spacing_and_cap() {
        let mut rng = ViewRng::seeded(3);
        let mut ribbon = TrailRibbon {
            shell: Entity::PLACEHOLDER,
            shot: None,
            points: VecDeque::new(),
            consumed: 0,
            arc: 0.0,
            observed_ricochets: 0,
            pending_bounces: VecDeque::new(),
            #[cfg(test)]
            mesh_evidence_pending: VecDeque::new(),
        };
        capture_point(&mut ribbon, Vec3::ZERO, 0.0, 0, 0, &mut rng);
        // Too close: filtered.
        capture_point(
            &mut ribbon,
            Vec3::X * (TRAIL_MIN_SPACING * 0.5),
            0.0,
            1,
            0,
            &mut rng,
        );
        assert_eq!(ribbon.points.len(), 1, "sub-spacing point must be dropped");
        // Far enough: captured, arc advanced by the true gap.
        capture_point(
            &mut ribbon,
            Vec3::X * (TRAIL_MIN_SPACING * 1.5),
            0.0,
            2,
            0,
            &mut rng,
        );
        assert_eq!(ribbon.points.len(), 2);
        assert_eq!(ribbon.points[1].arc, TRAIL_MIN_SPACING * 1.5);

        // A disconnected authority re-seed is captured even when spatially close, and its fictional
        // correction gap contributes no arc length.
        let arc_before_reseed = ribbon.arc;
        assert!(capture_point(
            &mut ribbon,
            Vec3::X * (TRAIL_MIN_SPACING * 1.5 + 0.5),
            0.0,
            3,
            1,
            &mut rng,
        ));
        assert_eq!(ribbon.points.back().unwrap().segment, 1);
        assert_eq!(ribbon.arc, arc_before_reseed);

        // The cap holds under a long flight.
        for i in 0..TRAIL_MAX_POINTS + 20 {
            capture_point(
                &mut ribbon,
                Vec3::X * (i as f32 + 2.0) * TRAIL_MIN_SPACING,
                0.0,
                i + 4,
                1,
                &mut rng,
            );
        }
        assert_eq!(ribbon.points.len(), TRAIL_MAX_POINTS);
    }

    /// The live wiring on real ECS systems: a DRESSED shell (`ShellVisual`) grows a ribbon whose
    /// mesh has geometry; an undressed MG round never does; and once the shell despawns the ribbon
    /// outlives it only until its smoke fully expires.
    #[test]
    fn trails_attach_follow_and_outlive_the_shell() {
        let mut app = App::new();
        app.init_resource::<TrailRing>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<VfxTrailMaterial>>()
            .init_resource::<Time>()
            .insert_resource(ViewRng::seeded(9))
            .add_message::<TrailStationMeshEvidence>()
            .add_observer(attach_trail)
            .add_observer(flush_removed_shell_path)
            .add_systems(Update, update_trails);
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

        // A dressed shell mid-flight, and an undressed MG round (ShellPath only).
        let shell = app
            .world_mut()
            .spawn((
                ShellPath {
                    points: vec![Vec3::ZERO, Vec3::X * 20.0, Vec3::X * 40.0],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                ShellVisual,
                Transform::from_translation(Vec3::X * 50.0),
            ))
            .id();
        app.world_mut().spawn((
            ShellPath {
                points: vec![Vec3::ZERO, Vec3::X * 20.0],
                segment_starts: Vec::new(),
            },
            PenetrationMarks::default(),
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

        // The final fixed tick adds a disconnected bounce segment and despawns BEFORE another trail
        // update. The removal observer must flush those last points while ShellPath is still readable.
        let bounce = Vec3::X * 80.0;
        {
            let mut path = app.world_mut().get_mut::<ShellPath>(shell).unwrap();
            let start = path.points.len();
            path.segment_starts.push(start);
            path.points.extend([bounce, Vec3::X * 100.0]);
        }
        app.world_mut()
            .get_mut::<PenetrationMarks>(shell)
            .unwrap()
            .ricochets
            .push(bounce);

        // Impact: the shell despawns; the ribbon must survive it with the final segment consumed...
        app.world_mut().despawn(shell);
        let ribbon = app.world().get::<TrailRibbon>(trails[0]).unwrap();
        assert_eq!(ribbon.consumed, 6, "removal flushed the final raw path");
        assert!(
            ribbon
                .points
                .iter()
                .any(|point| point.segment == 1 && point.source_index > 4),
            "the renderer captured a station strictly after the final bounce"
        );
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
