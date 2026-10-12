//! The corner's stage: the struck tank's stage view dressed for the frame, and the record drawn
//! over it — the line as wide as the calibre, a ring where it crossed each plate, the mark it left,
//! every spall cone as a translucent reach surface, and each box tinted by the dose it took. Rebuilt
//! when the corner's record or a look setting changes; animated every frame from the corner's
//! clock alone, so a scrub lands on the same picture.
//!
//! Frames: the see-through hull (additive rim-lit glass, the struck plates a neutral ghost, the
//! boxes from the record), the plate slice (the tank's own materials, the eye's side of a vertical
//! plane through the chord dissolved away), and the outside view (the tank as built).

use std::collections::HashMap;

use bevy::camera::ScalingMode;
use bevy::camera::{Hdr, RenderTarget};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;

use super::field::reach_mesh;
use super::material::{GlowMaterial, SliceCut, SliceMaterial, SliceParams};
use super::plan::{Plan, Reach};
use super::timeline::{self, CornerKind};
use super::view::{Proxy, ViewMesh, ViewRoot};
use super::{Corner, DoseScale, Library, Run, Settings};
use crate::render_policy::{self, CameraProfile, LightProfile, VisualScope};

/// The corner's clear colour.
const BACKGROUND: Color = Color::srgb_u8(0x0F, 0x13, 0x18);
/// The glass hull's colour and the line's.
const GLASS: Color = Color::srgb_u8(0x9F, 0xB0, 0xC0);
const LINE: Color = Color::srgb_u8(0xFF, 0xF4, 0xE0);
/// The line's emission at full capability, for the bloom.
const LINE_GLOW: f32 = 1.8;
/// A ring at a crossing: a calibre across, this thick (fraction of its radius).
const RING_THICKNESS: f32 = 0.12;
const RING_GLOW: f32 = 2.0;
/// The spall cone is drawn as this many nested copies of its reach surface, scaled about its
/// origin, so its density reads as depth; the on-axis brightness of all of them together is
/// [`CONE_PEAK`] at the top of the dose scale.
const CONE_SHELLS: usize = 4;
const CONE_PEAK: f32 = 0.35;
/// The shells' emission gain over [`CONE_PEAK`], for the bloom: a diagram choice.
const CONE_GAIN: f32 = 4.0;
/// The reach surface's rim glow at the top of the dose scale.
const CONE_RIM: f32 = 1.4;
/// A dosed box's emission at full fill.
const BOX_GLOW: f32 = 1.6;
/// The lost-box fill (victim glass): `#2A2F36` at 70 %.
const LOST_FILL: Color = Color::srgb_u8(0x2A, 0x2F, 0x36);
const OUTLINE: Color = Color::srgb_u8(0x8A, 0x96, 0xA3);
/// The slice's cut face.
const CUT: Color = Color::srgb_u8(0xC8, 0xCD, 0xD3);
/// The corner camera's vertical field (deg) in perspective.
const CORNER_FOV_DEG: f32 = 30.0;
/// The outside view frames this much of each leg either side of the contact (m).
const LEG_FRAME_M: f32 = 1.5;
/// A slice cut along a shot within this angle (deg) of vertical spans no vertical plane, and is
/// cut through the shot and the struck face's normal instead.
const SLICE_VERTICAL_FALLBACK_DEG: f32 = 10.0;
/// The slice frames this far (m) before the first crossing and past the stop, and at least this
/// tall across the shot.
const SLICE_LEAD_M: f32 = 1.0;
const SLICE_MIN_HEIGHT_M: f32 = 1.0;

#[derive(Component)]
pub struct StageCamera;

/// Everything [`rebuild`] spawns for one record.
#[derive(Component)]
pub(super) struct StageItem;

/// One leg of the line between arc lengths `from` and `to`.
#[derive(Component)]
pub(super) struct LineSeg {
    from: f32,
    to: f32,
    brightness: f32,
}

/// A mark that appears at an arc length of the line (`None`: at a fixed corner time).
#[derive(Component)]
pub(super) struct MarkItem {
    at_arc: Option<f32>,
    at_c: f32,
}

/// One copy of cone `cone`'s reach surface at scale `k`; `rim` marks the outer surface's glow.
#[derive(Component)]
pub(super) struct ConeShell {
    cone: usize,
    k: f32,
    rim: bool,
    peak: f32,
}

/// Box `index` of the plan, tinted by its dose.
#[derive(Component)]
pub(super) struct BoxTint {
    index: usize,
}

#[derive(GizmoConfigGroup, Default, Reflect)]
pub struct StageGizmos;

/// What the stage is built for; a change rebuilds it.
#[derive(Clone, PartialEq)]
pub(super) struct StageKey {
    turn: usize,
    record: usize,
    kind: CornerKind,
    settings: Settings,
}

/// The built stage's shared state.
#[derive(Resource, Default)]
pub struct Stage {
    key: Option<StageKey>,
    /// The slice plane: unit normal toward the eye, and a point on it.
    plane: Option<(Vec3, Vec3)>,
    glass: Handle<GlowMaterial>,
    ghost: Handle<GlowMaterial>,
    slices: HashMap<AssetId<StandardMaterial>, Handle<SliceMaterial>>,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Stage>().insert_gizmo_config(
        StageGizmos,
        GizmoConfig {
            render_layers: render_policy::pen_cam_gizmo_layers(),
            // Outlines draw over the tank, so a dosed box buried in an opaque hull still shows.
            depth_bias: -1.0,
            line: GizmoLineConfig {
                width: 1.5,
                ..default()
            },
            ..default()
        },
    );
}

pub fn spawn(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut glows: ResMut<Assets<GlowMaterial>>,
    mut stage: ResMut<Stage>,
) {
    let placeholder = images.add(Image::new_target_texture(
        16,
        10,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: -1,
            is_active: false,
            clear_color: ClearColorConfig::Custom(BACKGROUND),
            ..default()
        },
        RenderTarget::Image(placeholder.into()),
        Hdr,
        Bloom::NATURAL,
        Projection::Perspective(PerspectiveProjection {
            fov: CORNER_FOV_DEG.to_radians(),
            ..default()
        }),
        Transform::from_xyz(6.0, 4.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
        CameraProfile::PenCamInset,
        StageCamera,
    ));
    for (illuminance, toward) in [
        (3500.0, Vec3::new(0.4, 1.0, 0.3)),
        (1200.0, Vec3::new(-0.6, 0.3, -0.5)),
    ] {
        commands.spawn((
            DirectionalLight {
                illuminance,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_translation(toward).looking_at(Vec3::ZERO, Vec3::Y),
            LightProfile::PenCamKey,
        ));
    }
    stage.glass = glows.add(glass_material(0));
    stage.ghost = glows.add(GlowMaterial::blended(GLASS.into(), 0.18));
}

/// The glass hull, by look-test variant: the specified rim `0.03 + 0.5·(1 − |n·v|)³`, a
/// stronger rim, or flat.
fn glass_material(variant: u8) -> GlowMaterial {
    let glass = GlowMaterial::additive(GLASS.into(), 0.03);
    match variant {
        0 => glass.with_rim(0.03, 0.5, 3.0),
        1 => glass.with_rim(0.02, 0.9, 2.0),
        _ => glass.with_rim(0.08, 0.0, 1.0),
    }
}

/// Rebuild the stage when the corner's record, frame or a look setting changes.
pub fn rebuild(
    mut commands: Commands,
    corner: Res<Corner>,
    settings: Res<Settings>,
    library: Res<Library>,
    mut stage: ResMut<Stage>,
    items: Query<Entity, With<StageItem>>,
    tank: Query<(Entity, &ViewMesh), Without<Proxy>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut glows: ResMut<Assets<GlowMaterial>>,
    standard: Res<Assets<StandardMaterial>>,
    mut slices: ResMut<Assets<SliceMaterial>>,
) {
    let key = corner.showing.map(|showing| StageKey {
        turn: showing.turn,
        record: showing.record,
        kind: showing.kind,
        settings: settings.clone(),
    });
    if key == stage.key {
        return;
    }
    stage.key = key.clone();
    for entity in &items {
        commands.entity(entity).despawn();
    }
    let Some(key) = key else {
        return;
    };
    let plan = &library.records[key.record].plan;
    let kind = key.kind;
    if let Some(mut glass) = glows.get_mut(&stage.glass) {
        *glass = glass_material(settings.glass);
    }
    stage.plane = (kind == CornerKind::Slice).then(|| slice_plane(plan));

    // The tank, dressed for the frame.
    let struck = |mesh: &ViewMesh| {
        library.records[key.record]
            .report
            .plates
            .iter()
            .any(|plate| mesh.is(&plate.name))
    };
    for (entity, mesh) in &tank {
        if mesh.root != ViewRoot::Stage {
            continue;
        }
        let mut e = commands.entity(entity);
        e.remove::<(
            MeshMaterial3d<StandardMaterial>,
            MeshMaterial3d<GlowMaterial>,
            MeshMaterial3d<SliceMaterial>,
        )>();
        match kind {
            CornerKind::Glass | CornerKind::Victim => {
                let material = if struck(mesh) && settings.glass != 2 {
                    stage.ghost.clone()
                } else {
                    stage.glass.clone()
                };
                e.insert((MeshMaterial3d(material), Visibility::Inherited));
            }
            CornerKind::Slice => {
                let (normal, point) = stage.plane.unwrap_or((Vec3::Z, Vec3::ZERO));
                let params = SliceParams {
                    plane: normal.extend(normal.dot(point)),
                    cut_color: LinearRgba::from(CUT).to_vec4(),
                    params: Vec4::new(0.0, 8.0, 0.3, 0.0),
                };
                let handle = stage
                    .slices
                    .entry(mesh.original.id())
                    .or_insert_with(|| {
                        let mut base = standard.get(&mesh.original).cloned().unwrap_or_default();
                        base.cull_mode = None;
                        base.double_sided = true;
                        slices.add(SliceMaterial {
                            base,
                            extension: SliceCut { params },
                        })
                    })
                    .clone();
                if let Some(mut material) = slices.get_mut(&handle) {
                    material.extension.params = params;
                }
                e.insert((MeshMaterial3d(handle), Visibility::Inherited));
            }
            CornerKind::Ricochet | CornerKind::Damaged => {
                e.insert((MeshMaterial3d(mesh.original.clone()), Visibility::Inherited));
            }
        }
    }

    let scope = VisualScope::PEN_CAM_STAGE;
    let clip = stage.plane;
    // The line, one cylinder per leg inside the drawn span.
    let cylinder = meshes.add(Cylinder::new(0.5, 1.0));
    let line = &plan.line;
    for i in 1..line.points.len() {
        let (from, to) = (
            line.arc[i - 1].max(plan.start_arc),
            line.arc[i].min(plan.end_arc),
        );
        if to <= from {
            continue;
        }
        // In the slice the line lies in the cut, so it is dimmed for the band to read through it.
        let brightness = plan.brightness_at(0.5 * (from + to))
            * if kind == CornerKind::Slice { 0.45 } else { 1.0 };
        let mut material = GlowMaterial::additive(LINE.into(), LINE_GLOW * brightness);
        material.set_fade(0.0);
        commands.spawn((
            Mesh3d(cylinder.clone()),
            MeshMaterial3d(glows.add(material)),
            Transform::IDENTITY,
            Visibility::Hidden,
            LineSeg {
                from,
                to,
                brightness,
            },
            scope,
            StageItem,
        ));
    }

    // Rings where the line crossed each plate; the gouge of a bounce; the crater at a stop.
    let ring = meshes.add(Torus::new(1.0 - RING_THICKNESS, 1.0 + RING_THICKNESS));
    let mut mark = |commands: &mut Commands, transform: Transform, at_arc, at_c, glow: f32| {
        let mut material = GlowMaterial::additive(LINE.into(), glow);
        material.set_fade(0.0);
        commands.spawn((
            Mesh3d(ring.clone()),
            MeshMaterial3d(glows.add(material)),
            transform,
            Visibility::Hidden,
            MarkItem { at_arc, at_c },
            scope,
            StageItem,
        ));
    };
    let radius = plan.calibre * 0.5;
    if matches!(
        kind,
        CornerKind::Glass | CornerKind::Victim | CornerKind::Damaged
    ) {
        for crossing in &plan.crossings {
            let travel = line.direction_at(crossing.entry_arc + 1e-3);
            let first = (crossing.entry_arc - plan.contact_arc).abs() < 1e-3;
            let transform = if first {
                ring_on_face(crossing.entry, plan.impact_normal, travel, radius)
            } else {
                ring_across(crossing.entry, travel, radius)
            };
            mark(
                &mut commands,
                transform,
                Some(crossing.entry_arc),
                0.0,
                RING_GLOW,
            );
            if let Some(exit) = crossing.exit {
                let exit_arc = line.arc_of(exit);
                mark(
                    &mut commands,
                    ring_across(exit, travel, radius),
                    Some(exit_arc),
                    0.0,
                    RING_GLOW,
                );
            }
        }
    }
    if let Some(bounce) = &plan.bounce {
        mark(
            &mut commands,
            ring_on_face(bounce.point, bounce.normal, bounce.incoming, radius),
            None,
            0.0,
            RING_GLOW,
        );
    }
    if let Some(stop) = plan.stop {
        let (transform, at_arc, at_c) = match (kind, stage.plane) {
            (CornerKind::Slice, Some((normal, _))) => (
                ring_facing(stop, normal, radius),
                None,
                timeline::SLICE_CRATER.0,
            ),
            _ => (
                ring_across(stop, line.direction_at(plan.end_arc), radius),
                Some(plan.end_arc),
                0.0,
            ),
        };
        mark(&mut commands, transform, at_arc, at_c, RING_GLOW * 1.5);
    }

    // Spall cones: nested reach surfaces and a rim-lit outer one, clipped by the slice plane.
    let cone_color: LinearRgba = timeline::dose_ramp(0.75);
    for (index, cone) in plan.cones.iter().enumerate() {
        if matches!(kind, CornerKind::Ricochet) {
            break;
        }
        let strength = timeline::absolute_dose(cone.strength.max(1e-3));
        let mesh = meshes.add(reach_mesh(cone));
        for shell in 0..=CONE_SHELLS {
            let rim = shell == CONE_SHELLS;
            let k = if rim {
                1.0
            } else {
                (shell + 1) as f32 / CONE_SHELLS as f32
            };
            let peak = if rim {
                CONE_RIM * strength
            } else {
                CONE_GAIN * CONE_PEAK * strength / CONE_SHELLS as f32
            };
            let mut material = if rim {
                GlowMaterial::additive(cone_color, 0.0).with_rim(0.0, peak, 2.5)
            } else {
                GlowMaterial::additive(cone_color, peak).with_vertex_color()
            };
            material.set_clip(clip);
            material.set_fade(0.0);
            commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(glows.add(material)),
                Transform::from_translation(cone.origin).with_scale(Vec3::splat(1e-4)),
                Visibility::Hidden,
                ConeShell {
                    cone: index,
                    k,
                    rim,
                    peak,
                },
                scope,
                StageItem,
            ));
        }
    }

    // Boxes: dose tint where the shot dosed one; the victim's lost boxes in the state tint.
    let cube = meshes.add(Cuboid::new(2.0, 2.0, 2.0));
    for (index, b) in plan.boxes.iter().enumerate() {
        let material = match kind {
            CornerKind::Victim if b.lost => {
                GlowMaterial::blended(LOST_FILL.into(), 0.7).with_hatch()
            }
            CornerKind::Glass | CornerKind::Damaged if b.dose() > 0.0 => {
                let x = match settings.dose {
                    DoseScale::Absolute => timeline::absolute_dose(b.dose()),
                    DoseScale::Relative => timeline::relative_dose(b.dose(), b.count),
                };
                GlowMaterial::additive(timeline::dose_ramp(x), BOX_GLOW * (0.25 + 0.75 * x))
            }
            _ => continue,
        };
        let mut material = material;
        material.set_fade(0.0);
        commands.spawn((
            Mesh3d(cube.clone()),
            MeshMaterial3d(glows.add(material)),
            Transform::from_translation(b.center)
                .with_rotation(b.rotation)
                .with_scale(b.half),
            Visibility::Hidden,
            BoxTint { index },
            scope,
            StageItem,
        ));
    }
}

/// The ring a calibre across on a face, stretched along the line's track on it by
/// `1 / cos(incidence)`.
fn ring_on_face(point: Vec3, normal: Vec3, travel: Vec3, radius: f32) -> Transform {
    let normal = normal.normalize_or(Vec3::Y);
    let along = (travel - normal * travel.dot(normal))
        .try_normalize()
        .unwrap_or_else(|| normal.any_orthonormal_vector());
    let cos = (-travel).dot(normal).abs().max(0.2);
    let across = along.cross(normal);
    Transform::from_translation(point + normal * 0.003)
        .with_rotation(Quat::from_mat3(&Mat3::from_cols(along, normal, across)))
        .with_scale(Vec3::new(radius / cos, radius, radius))
}

/// A ring a calibre across, square to the line.
fn ring_across(point: Vec3, travel: Vec3, radius: f32) -> Transform {
    ring_facing(point, travel, radius)
}

/// A ring a calibre across whose axis is `axis`.
fn ring_facing(point: Vec3, axis: Vec3, radius: f32) -> Transform {
    Transform::from_translation(point)
        .with_rotation(Quat::from_rotation_arc(Vec3::Y, axis.normalize_or(Vec3::Y)))
        .with_scale(Vec3::splat(radius))
}

/// The slice plane: vertical through the stopping chord, its normal horizontal and toward the
/// side from which the round travels left to right. A near-vertical chord is cut through the
/// chord and the struck face's normal instead.
fn slice_plane(plan: &Plan) -> (Vec3, Vec3) {
    let d = plan.chord_direction();
    let point = plan.stop.unwrap_or(plan.impact_point);
    let vertical = d.y.abs() > SLICE_VERTICAL_FALLBACK_DEG.to_radians().cos();
    let mut normal = if vertical {
        d.cross(plan.impact_normal)
            .try_normalize()
            .unwrap_or_else(|| d.any_orthonormal_vector())
    } else {
        d.cross(Vec3::Y).normalize()
    };
    // Looking along −normal with +Y up puts `(−normal) × up` on the right.
    if (-normal).cross(Vec3::Y).dot(d) < 0.0 {
        normal = -normal;
    }
    (normal, point)
}

/// The arc length of the line's head at corner time `c`.
fn head_arc(plan: &Plan, kind: CornerKind, c: f32, reduce: bool) -> f32 {
    let (from, to) = kind.line_window();
    let p = timeline::progress(c, from, to - from);
    if reduce {
        return if c >= from {
            plan.end_arc
        } else {
            start_of(plan, kind)
        };
    }
    let start = start_of(plan, kind);
    let eased = match kind {
        CornerKind::Glass | CornerKind::Victim => p,
        _ => timeline::ease_travel(p),
    };
    start + (plan.end_arc - start) * eased
}

/// Where the head stands before the line window opens: the outside view already has the approach
/// drawn to the face.
fn start_of(plan: &Plan, kind: CornerKind) -> f32 {
    match kind {
        CornerKind::Ricochet | CornerKind::Damaged => plan.contact_arc,
        _ => plan.start_arc,
    }
}

/// The corner time at which the head reaches arc `s`.
fn time_at_arc(plan: &Plan, kind: CornerKind, s: f32, reduce: bool) -> f32 {
    let (from, to) = kind.line_window();
    if s <= start_of(plan, kind) {
        return 0.0;
    }
    let (mut lo, mut hi) = (from, to);
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if head_arc(plan, kind, mid, reduce) >= s {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// Animate the stage from the corner's clock and frame the corner camera.
pub fn animate(
    corner: Res<Corner>,
    settings: Res<Settings>,
    library: Res<Library>,
    run: Res<Run>,
    stage: Res<Stage>,
    mut glows: ResMut<Assets<GlowMaterial>>,
    mut slices: ResMut<Assets<SliceMaterial>>,
    mut lines: Query<
        (
            &LineSeg,
            &MeshMaterial3d<GlowMaterial>,
            &mut Transform,
            &mut Visibility,
        ),
        (Without<MarkItem>, Without<ConeShell>, Without<StageCamera>),
    >,
    mut marks: Query<
        (&MarkItem, &MeshMaterial3d<GlowMaterial>, &mut Visibility),
        (Without<LineSeg>, Without<ConeShell>, Without<BoxTint>),
    >,
    mut shells: Query<
        (
            &ConeShell,
            &MeshMaterial3d<GlowMaterial>,
            &mut Transform,
            &mut Visibility,
        ),
        (Without<LineSeg>, Without<MarkItem>, Without<StageCamera>),
    >,
    mut boxes: Query<
        (&BoxTint, &MeshMaterial3d<GlowMaterial>, &mut Visibility),
        (Without<LineSeg>, Without<MarkItem>, Without<ConeShell>),
    >,
    mut camera: Query<
        (&mut Camera, &mut Transform, &mut Projection),
        (With<StageCamera>, Without<LineSeg>, Without<ConeShell>),
    >,
    mut gizmos: Gizmos<StageGizmos>,
) {
    let Ok((mut camera, mut eye, mut projection)) = camera.single_mut() else {
        return;
    };
    let Some(showing) = corner.showing else {
        camera.is_active = false;
        return;
    };
    camera.is_active = true;
    let plan = &library.records[showing.record].plan;
    let kind = showing.kind;
    let c = showing.c;
    let reduce = settings.reduce_motion;
    let fade_from = |start: f32| {
        let span = if reduce {
            timeline::REDUCED_FADE_S
        } else {
            0.12
        };
        timeline::progress(c, start, span)
    };
    let head = head_arc(plan, kind, c, reduce);
    let line_fade = if reduce {
        fade_from(kind.line_window().0)
    } else {
        1.0
    };

    for (seg, material, mut transform, mut visibility) in &mut lines {
        let to = seg.to.min(head);
        if to <= seg.from {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        let (a, b) = (plan.line.at(seg.from), plan.line.at(to));
        let span = b - a;
        let length = span.length();
        if length < 1e-5 {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        *transform = Transform::from_translation(0.5 * (a + b))
            .with_rotation(Quat::from_rotation_arc(Vec3::Y, span / length))
            .with_scale(Vec3::new(plan.calibre, length, plan.calibre));
        if let Some(mut m) = glows.get_mut(&material.0) {
            m.set_fade(line_fade);
            m.set_intensity(LINE_GLOW * seg.brightness);
        }
        visibility.set_if_neq(Visibility::Inherited);
    }

    for (mark, material, mut visibility) in &mut marks {
        let start = match mark.at_arc {
            Some(arc) => time_at_arc(plan, kind, arc, reduce),
            None => mark.at_c,
        };
        let fade = fade_from(start);
        if let Some(mut m) = glows.get_mut(&material.0) {
            m.set_fade(fade);
        }
        visibility.set_if_neq(if fade > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }

    let cone_start = |index: usize| time_at_arc(plan, kind, plan.cones[index].arc, reduce);
    for (shell, material, mut transform, mut visibility) in &mut shells {
        let cone = &plan.cones[shell.cone];
        let start = cone_start(shell.cone);
        let (grow, fade) = if reduce {
            (1.0, fade_from(start))
        } else {
            let g = timeline::ease_enter(timeline::progress(c, start, timeline::CONE_GROW_S));
            (g, if c >= start { 1.0 } else { 0.0 })
        };
        if fade <= 0.0 || grow <= 0.0 {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        *transform = Transform::from_translation(cone.origin)
            .with_scale(Vec3::splat((shell.k * grow).max(1e-4)));
        if let Some(mut m) = glows.get_mut(&material.0) {
            m.set_fade(fade);
            if !shell.rim {
                m.set_intensity(shell.peak);
            }
        }
        visibility.set_if_neq(Visibility::Inherited);
    }

    for (tint, material, mut visibility) in &mut boxes {
        let b = &plan.boxes[tint.index];
        let start = match (kind, b.reached_at) {
            (CornerKind::Victim, _) => 0.0,
            (_, Some(Reach::Line(arc))) => time_at_arc(plan, kind, arc, reduce),
            (_, Some(Reach::Cone(index))) => cone_start(index) + timeline::CONE_GROW_S,
            (_, None) => f32::INFINITY,
        };
        let fill = if reduce {
            fade_from(start)
        } else {
            timeline::ease_enter(timeline::progress(c, start, timeline::BOX_FILL_S))
        };
        if let Some(mut m) = glows.get_mut(&material.0) {
            m.set_fade(fill);
        }
        visibility.set_if_neq(if fill > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        // A drawn box is outlined over the tank: a dosed one on the glass faintly, a lost one
        // (victim glass) in full, a dosed one in the outside view in its tint, since the opaque
        // hull may bury it.
        if fill > 0.0 {
            let color = match kind {
                CornerKind::Victim => OUTLINE,
                CornerKind::Damaged => {
                    let x = match settings.dose {
                        DoseScale::Absolute => timeline::absolute_dose(b.dose()),
                        DoseScale::Relative => timeline::relative_dose(b.dose(), b.count),
                    };
                    Color::from(timeline::dose_ramp(x))
                }
                _ => OUTLINE.with_alpha(0.45),
            };
            gizmos.cube(
                Transform::from_translation(b.center)
                    .with_rotation(b.rotation)
                    .with_scale(b.half * 2.0),
                color.with_alpha(color.alpha() * fill),
            );
        }
    }

    if settings.axis_line {
        for (index, cone) in plan.cones.iter().enumerate() {
            if c >= cone_start(index) {
                let reach = cone.rays.iter().map(|r| r.reach).fold(0.0, f32::max);
                gizmos.line(
                    cone.origin,
                    cone.origin + cone.axis * reach,
                    Color::srgb(0.95, 0.95, 0.95),
                );
            }
        }
    }

    // The slice's reveal.
    if let Some((normal, point)) = stage.plane {
        let (from, to) = timeline::SLICE_REVEAL;
        let removal = timeline::ease_enter(timeline::progress(c, from, to - from));
        for handle in stage.slices.values() {
            if let Some(mut m) = slices.get_mut(handle) {
                m.extension.params.plane = normal.extend(normal.dot(point));
                m.extension.params.params.x = removal;
            }
        }
    }

    let (want, want_projection) = framing(plan, kind, c, &settings, &stage, run.orbit);
    if *eye != want {
        *eye = want;
    }
    *projection = want_projection;
}

/// The corner camera for frame `kind` at corner time `c`.
fn framing(
    plan: &Plan,
    kind: CornerKind,
    c: f32,
    settings: &Settings,
    stage: &Stage,
    orbit: Vec2,
) -> (Transform, Projection) {
    let half_fov = 0.5 * CORNER_FOV_DEG.to_radians();
    let perspective = || {
        Projection::Perspective(PerspectiveProjection {
            fov: CORNER_FOV_DEG.to_radians(),
            ..default()
        })
    };
    let motion = |rate_deg: f32, from: f32| {
        if settings.reduce_motion {
            0.0
        } else {
            rate_deg.to_radians() * (c - from).max(0.0)
        }
    };
    // A three-quarter view from the shot's side, orbiting about the vertical at `rate` (deg/s).
    let three_quarter = |points: &[Vec3], rate: f32, from: f32| {
        let (center, radius) = enclose(points);
        let d = plan.line.direction_at(plan.contact_arc);
        let flat = Vec3::new(d.x, 0.0, d.z)
            .try_normalize()
            .unwrap_or(Vec3::NEG_Z);
        let yaw = 55f32.to_radians() + motion(rate, from) + orbit.x;
        let side = Quat::from_rotation_y(yaw) * -flat;
        let pitch = (0.5 + orbit.y).clamp(-1.2, 1.4);
        let direction = (side + Vec3::Y * pitch).normalize();
        let distance = radius.max(1.0) * 1.1 / half_fov.sin();
        (
            Transform::from_translation(center + direction * distance).looking_at(center, Vec3::Y),
            perspective(),
        )
    };
    match kind {
        CornerKind::Glass | CornerKind::Victim => {
            // The whole hull: the record's path inside the tank its boxes span.
            let mut points = plan.framed_points();
            points.extend(plan.boxes.iter().map(|b| b.center));
            three_quarter(&points, timeline::ORBIT_DEG_S, 0.0)
        }
        CornerKind::Damaged => {
            let incoming = plan.line.direction_at(plan.contact_arc - 1e-3);
            let mut points = vec![
                plan.impact_point - incoming * LEG_FRAME_M,
                plan.line
                    .at(plan.end_arc.min(plan.contact_arc + LEG_FRAME_M)),
                plan.impact_point,
            ];
            points.extend(
                plan.boxes
                    .iter()
                    .filter(|b| b.dose() > 0.0)
                    .map(|b| b.center),
            );
            three_quarter(&points, timeline::DRIFT_DEG_S, kind.hold_from())
        }
        CornerKind::Slice => {
            let (normal, _) = stage.plane.unwrap_or((Vec3::X, Vec3::ZERO));
            let d = plan.chord_direction();
            let start = plan
                .crossings
                .first()
                .map_or(plan.impact_point, |crossing| crossing.entry)
                - d * SLICE_LEAD_M;
            let end = plan.stop.unwrap_or(plan.line.at(plan.end_arc)) + d * (SLICE_LEAD_M * 0.6);
            let up = (Vec3::Y - normal * normal.y)
                .try_normalize()
                .unwrap_or_else(|| normal.any_orthonormal_vector());
            let right = (-normal).cross(up);
            let center = 0.5 * (start + end);
            let width = (end - start).dot(right).abs();
            let height = ((end - start).dot(up).abs() + 6.0 * plan.calibre)
                .max(SLICE_MIN_HEIGHT_M)
                .max(width / 1.6);
            let width = width.max(height * 1.6);
            let yawed = Quat::from_axis_angle(up, orbit.x) * normal;
            if settings.slice_ortho {
                (
                    Transform::from_translation(center + yawed * 10.0).looking_to(-yawed, up),
                    Projection::Orthographic(OrthographicProjection {
                        scaling_mode: ScalingMode::AutoMin {
                            min_width: width * 1.1,
                            min_height: height * 1.1,
                        },
                        ..OrthographicProjection::default_3d()
                    }),
                )
            } else {
                let distance = 0.55 * height.max(width / 1.6) / half_fov.tan();
                (
                    Transform::from_translation(center + yawed * distance).looking_to(-yawed, up),
                    perspective(),
                )
            }
        }
        CornerKind::Ricochet => {
            let Some(bounce) = plan.bounce.as_ref() else {
                return three_quarter(&plan.framed_points(), timeline::DRIFT_DEG_S, 0.0);
            };
            // The eye on the normal of the plane through both legs, so the angle shows at its
            // true size; above the plane when the legs are level.
            let mut normal = bounce
                .incoming
                .cross(bounce.outgoing)
                .try_normalize()
                .unwrap_or_else(|| {
                    bounce
                        .incoming
                        .cross(Vec3::Y)
                        .try_normalize()
                        .unwrap_or(Vec3::X)
                });
            if normal.dot(Vec3::Y) < 0.0
                || (normal.y.abs() < 1e-3 && normal.dot(bounce.normal) < 0.0)
            {
                normal = -normal;
            }
            let points = [
                bounce.point - bounce.incoming * LEG_FRAME_M,
                bounce.point + bounce.outgoing * LEG_FRAME_M,
                bounce.point,
            ];
            let (center, radius) = enclose(&points);
            // The struck face's normal points up the frame: the legs open above the face.
            let up = (bounce.normal - normal * bounce.normal.dot(normal))
                .try_normalize()
                .unwrap_or_else(|| normal.any_orthonormal_vector());
            let drift = motion(timeline::DRIFT_DEG_S, kind.hold_from());
            let direction = Quat::from_axis_angle(up, drift + orbit.x) * normal;
            let distance = radius.max(0.6) * 1.15 / half_fov.sin();
            (
                Transform::from_translation(center + direction * distance).looking_at(center, up),
                perspective(),
            )
        }
    }
}

/// The centre and radius of a sphere around `points`.
fn enclose(points: &[Vec3]) -> (Vec3, f32) {
    let (lo, hi) = points.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| {
        (lo.min(*p), hi.max(*p))
    });
    let center = 0.5 * (lo + hi);
    let radius = points
        .iter()
        .map(|p| p.distance(center))
        .fold(0.0, f32::max);
    (center, radius)
}
