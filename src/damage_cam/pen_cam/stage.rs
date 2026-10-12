//! The corner's stage: the struck tank's stage view dressed for the frame, and the record drawn
//! over it — the round as the game's shell model at the record's calibre, travelling the recorded
//! path nose first with a thin trail behind it, a ring where it crossed each plate or glanced off a
//! face, every spall cone as a translucent reach surface, and each component's real mesh tinted by
//! the dose it took.
//! The shell rests where the record ends; a round the record has leave the tank runs on out of the
//! frame.
//! Rebuilt when the corner's record or a look setting changes; animated every frame from the
//! corner's clock alone, so a scrub lands on the same picture.
//!
//! Frames: the see-through hull (additive rim-lit glass, the struck plates a neutral ghost, the
//! components the record names drawn in their own meshes), the plate slice (the tank's own materials, the eye's side of a vertical
//! plane through the chord dissolved away), and the outside view (the tank as built).
//!
//! [`spawn_glass`] is the entry point that draws a record over a tank: it takes the tank's hull
//! entity (whose frame is the record's) and the tank's own meshes of the components the record
//! names ([`component_meshes`]), and spawns every record element under them. [`animate`] plays
//! them from the corner's clock.

use std::collections::HashMap;

use bevy::camera::ScalingMode;
use bevy::camera::{Hdr, RenderTarget};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

use super::field::reach_mesh;
use super::material::{GlowMaterial, SliceCut, SliceMaterial, SliceParams};
use super::plan::{Plan, Reach};
use super::timeline::{self, CornerKind};
use super::view::{Proxy, ViewMesh, ViewRoot, Views};
use super::{Corner, DoseScale, Library, Run, Settings};
use crate::render_policy::{self, CameraProfile, LightProfile, VisualScope};

/// The corner's clear colour.
const BACKGROUND: Color = Color::srgb_u8(0x0F, 0x13, 0x18);
/// The glass hull's colour and the trail's.
const GLASS: Color = Color::srgb_u8(0x9F, 0xB0, 0xC0);
const LINE: Color = Color::srgb_u8(0xFF, 0xF4, 0xE0);
/// The trail's emission at full capability, for the bloom. TUNED.
const LINE_GLOW: f32 = 1.8;
/// The trail's width in calibres: a diagram choice, thin beside the shell it follows. TUNED.
const TRAIL_CALIBRES: f32 = 0.3;
/// The game's shell model, the one `ballistics::view` dresses the main-gun round with: origin at
/// the tip, body along +Z, so `looking_to(travel)` puts the nose forward.
const SHELL_SCENE: &str = "shell/shell.glb";
/// The model's authored diameter and length (m), MEASURED from its position bounds
/// (±0.044 × ±0.044 × 0..0.263); the model is scaled by calibre over the diameter.
const SHELL_MODEL_CALIBRE_M: f32 = 0.088;
const SHELL_MODEL_LENGTH_M: f32 = 0.263;
/// The framed radius is at most this many calibres (TUNED), about the stop or else the contact:
/// the main gun's whole-hull frame (7.0 m for 88 mm, DERIVED) is untouched, a 7.9 mm round's is
/// held to 0.63 m (DERIVED).
const FRAME_CALIBRES: f32 = 80.0;
/// A ring at a crossing: a calibre across, this thick (fraction of its radius). TUNED, as is its
/// glow.
const RING_THICKNESS: f32 = 0.12;
const RING_GLOW: f32 = 2.0;
/// The spall cone is drawn as this many nested copies of its reach surface, scaled about its
/// origin, so its density reads as depth; the on-axis brightness of all of them together is
/// [`CONE_PEAK`] at the top of the dose scale. TUNED, as are the cone's and the boxes' glows below.
const CONE_SHELLS: usize = 4;
const CONE_PEAK: f32 = 0.35;
/// The shells' emission gain over [`CONE_PEAK`], for the bloom: a diagram choice.
const CONE_GAIN: f32 = 4.0;
/// The reach surface's rim glow at the top of the dose scale.
const CONE_RIM: f32 = 1.4;
/// A dosed component's emission at full fill.
const BOX_GLOW: f32 = 1.6;
/// The lost-box fill (victim glass): `#2A2F36` at 70 %.
const LOST_FILL: Color = Color::srgb_u8(0x2A, 0x2F, 0x36);
/// The slice's cut face.
const CUT: Color = Color::srgb_u8(0xC8, 0xCD, 0xD3);
/// The corner camera's vertical field (deg) in perspective.
const CORNER_FOV_DEG: f32 = 30.0;
/// The outside view frames this much of each leg either side of the contact (m). This and every
/// other framing length below is taken through [`Plan::reach`], so a small calibre frames tighter.
/// TUNED, as are the slice's figures below.
const LEG_FRAME_M: f32 = 1.5;
/// A slice cut along a shot within this angle (deg) of vertical spans no vertical plane, and is
/// cut through the shot and the struck face's normal instead.
const SLICE_VERTICAL_FALLBACK_DEG: f32 = 10.0;
/// The slice frames this far (m) before the first crossing and past the stop (that shortened by
/// 0.6), and at least this tall across the shot.
const SLICE_LEAD_M: f32 = 1.0;
const SLICE_MIN_HEIGHT_M: f32 = 1.0;

#[derive(Component)]
pub struct StageCamera;

/// The round: one shell model, placed and scaled for the record on show.
#[derive(Component)]
pub(super) struct StageShell;

/// Everything [`rebuild`] spawns for one record.
#[derive(Component)]
pub(super) struct StageItem;

/// One leg of the trail between arc lengths `from` and `to`.
#[derive(Component)]
pub(super) struct LineSeg {
    from: f32,
    to: f32,
    brightness: f32,
}

/// A mark that appears when the shell's nose reaches an arc length of the line.
#[derive(Component)]
pub(super) struct MarkItem {
    at_arc: f32,
}

/// One copy of cone `cone`'s reach surface at scale `k`; `rim` marks the outer surface's glow.
#[derive(Component)]
pub(super) struct ConeShell {
    cone: usize,
    k: f32,
    rim: bool,
    peak: f32,
}

/// A copy of one mesh of component `index` of the plan, over the tank's own, tinted by its dose.
#[derive(Component)]
pub(super) struct BoxTint {
    index: usize,
}

/// One of a tank's own meshes that belongs to a component the record names.
#[derive(Clone, Debug)]
pub struct ComponentMesh {
    /// Index into [`Plan::boxes`].
    pub index: usize,
    /// The mesh's entity in the tank's hierarchy: the tint is drawn as its child.
    pub entity: Entity,
    pub mesh: Handle<Mesh>,
}

/// Which of a tank's meshes belong to which of the plan's components. Each mesh comes with the
/// names of its node chain, nearest first; it belongs to the component its nearest named node of
/// the plan's names is, so a component nested in another is its own.
pub fn component_meshes<'a>(
    plan: &Plan,
    meshes: impl IntoIterator<Item = (Entity, &'a [String], Handle<Mesh>)>,
) -> Vec<ComponentMesh> {
    meshes
        .into_iter()
        .filter_map(|(entity, names, mesh)| {
            let index = names
                .iter()
                .find_map(|name| plan.boxes.iter().position(|b| b.name == *name))?;
            Some(ComponentMesh {
                index,
                entity,
                mesh,
            })
        })
        .collect()
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
            // The axis line draws over the tank.
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
    asset_server: Res<AssetServer>,
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
    let shell: Handle<WorldAsset> =
        asset_server.load(GltfAssetLabel::Scene(0).from_asset(SHELL_SCENE));
    commands.spawn((
        WorldAssetRoot(shell),
        Transform::IDENTITY,
        Visibility::Hidden,
        VisualScope::PEN_CAM_STAGE,
        StageShell,
    ));
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
    views: Res<Views>,
    tank: Query<(Entity, &ViewMesh, &Mesh3d), Without<Proxy>>,
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
    let Some(hull) = views.hull[1] else {
        return;
    };
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
    for (entity, mesh, _) in &tank {
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

    let components = component_meshes(
        plan,
        tank.iter()
            .filter(|(_, mesh, _)| mesh.root == ViewRoot::Stage)
            .map(|(entity, mesh, handle)| (entity, mesh.names.as_slice(), handle.0.clone())),
    );
    spawn_glass(
        &mut commands,
        &mut meshes,
        &mut glows,
        plan,
        kind,
        &settings,
        hull,
        stage.plane,
        &components,
    );
}

/// Draw `plan`'s record over a tank in frame `kind`: the trail, the rings, the spall cones and the
/// dose tints. Every element but the tints is spawned under `hull`, an entity whose frame is the
/// record's (the struck hull's); each tint is a copy of one of `components`, the tank's own meshes
/// of the components the record names, spawned under that mesh. `clip` is the slice plane (unit
/// normal toward the eye, a point on it) the cones are cut by. Everything spawned is a
/// [`StageItem`], hidden until [`animate`] places it.
#[allow(clippy::too_many_arguments)]
pub fn spawn_glass(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    glows: &mut Assets<GlowMaterial>,
    plan: &Plan,
    kind: CornerKind,
    settings: &Settings,
    hull: Entity,
    clip: Option<(Vec3, Vec3)>,
    components: &[ComponentMesh],
) {
    let scope = VisualScope::PEN_CAM_STAGE;
    // The trail, one cylinder per leg of the path from the drawn span's start; a round that left
    // trails on past the span behind the shell.
    let cylinder = meshes.add(Cylinder::new(0.5, 1.0));
    let line = &plan.line;
    for i in 1..line.points.len() {
        let (from, to) = (line.arc[i - 1].max(plan.start_arc), line.arc[i]);
        if to <= from {
            continue;
        }
        // In the slice the trail lies in the cut, so it is dimmed for the band to read through it.
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
            ChildOf(hull),
        ));
    }

    // Rings where the round crossed each plate, and the gouge of a bounce. The stop is the shell's
    // nose, so it gets no ring.
    let ring = meshes.add(Torus::new(1.0 - RING_THICKNESS, 1.0 + RING_THICKNESS));
    let mut mark = |commands: &mut Commands, transform: Transform, at_arc, glow: f32| {
        let mut material = GlowMaterial::additive(LINE.into(), glow);
        material.set_fade(0.0);
        commands.spawn((
            Mesh3d(ring.clone()),
            MeshMaterial3d(glows.add(material)),
            transform,
            Visibility::Hidden,
            MarkItem { at_arc },
            scope,
            StageItem,
            ChildOf(hull),
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
            mark(commands, transform, crossing.entry_arc, RING_GLOW);
            if let Some(exit) = crossing.exit {
                let exit_arc = line.arc_of(exit);
                mark(
                    commands,
                    ring_across(exit, travel, radius),
                    exit_arc,
                    RING_GLOW,
                );
            }
        }
    }
    if let Some(bounce) = &plan.bounce {
        mark(
            commands,
            ring_on_face(bounce.point, bounce.normal, bounce.incoming, radius),
            bounce.arc,
            RING_GLOW,
        );
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
                ChildOf(hull),
            ));
        }
    }

    // Components, in their own meshes: dose tint where the shot dosed one; the victim's lost ones in
    // the state tint.
    for component in components {
        let index = component.index;
        let b = &plan.boxes[index];
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
            Mesh3d(component.mesh.clone()),
            MeshMaterial3d(glows.add(material)),
            Transform::IDENTITY,
            Visibility::Hidden,
            BoxTint { index },
            scope,
            StageItem,
            ChildOf(component.entity),
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
    Transform::from_translation(point)
        .with_rotation(Quat::from_rotation_arc(
            Vec3::Y,
            travel.normalize_or(Vec3::Y),
        ))
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

/// The on-screen speed of the shell (m of path per corner second): the drawn span over the frame's
/// travel window, constant in every frame. An eased run would have the shell slow in the air
/// before it strikes, which no record says.
fn travel_speed(plan: &Plan, kind: CornerKind) -> f32 {
    let (from, to) = kind.line_window();
    (plan.end_arc - plan.start_arc).max(1e-3) / (to - from)
}

/// The arc length of the shell's nose at corner time `c`. It crosses the drawn span in the travel
/// window and stops where the record's path ends; a round that left runs on along the path at the
/// same speed, out of the frame. Reduce motion puts it at the span's end when the window opens.
fn head_arc(plan: &Plan, kind: CornerKind, c: f32, reduce: bool) -> f32 {
    let (from, _) = kind.line_window();
    if reduce {
        return if c >= from {
            plan.end_arc
        } else {
            plan.start_arc
        };
    }
    let run = travel_speed(plan, kind) * (c - from).max(0.0);
    (plan.start_arc + run).min(plan.line.length())
}

/// The corner time at which the nose reaches arc `s`.
fn time_at_arc(plan: &Plan, kind: CornerKind, s: f32, reduce: bool) -> f32 {
    let (from, _) = kind.line_window();
    if reduce {
        return from;
    }
    from + (s - plan.start_arc).max(0.0) / travel_speed(plan, kind)
}

/// Whether the shell is drawn at corner time `c`: from the travel window on, until a round the
/// record has leave the tank runs off the end of its recorded path. Under reduce motion a round
/// that left is never drawn, since it would stand parked in the air at the span's end.
fn shell_drawn(plan: &Plan, kind: CornerKind, c: f32, reduce: bool) -> bool {
    let (from, _) = kind.line_window();
    if c < from {
        return false;
    }
    if !plan.left() {
        return true;
    }
    !reduce && head_arc(plan, kind, c, reduce) < plan.line.length() - 1e-4
}

/// The shell model's transform with its nose at arc `head`: the tip on the line, the body back
/// along the travel there, scaled from the model's calibre to the record's.
fn shell_transform(plan: &Plan, head: f32) -> Transform {
    Transform::from_translation(plan.line.at(head))
        .looking_to(plan.line.direction_at(head), Vec3::Y)
        .with_scale(Vec3::splat(plan.calibre / SHELL_MODEL_CALIBRE_M))
}

/// Where the trail ends: its arc along the line and its point. Behind a drawn shell it ends at the
/// model's tail, which on a leg just past a bend stands off the line behind the bend; otherwise at
/// the nose.
fn trail_end(plan: &Plan, head: f32, drawn: bool) -> (f32, Vec3) {
    if !drawn {
        return (head, plan.line.at(head));
    }
    let length = SHELL_MODEL_LENGTH_M * plan.calibre / SHELL_MODEL_CALIBRE_M;
    let tail = shell_transform(plan, head).transform_point(Vec3::Z * SHELL_MODEL_LENGTH_M);
    (head - length, tail)
}

/// The drawn extent of the trail leg between arcs `from` and `to`, cut at the trail's end: the
/// leg holding that end runs to its point.
fn trail_span(plan: &Plan, from: f32, to: f32, end: (f32, Vec3)) -> Option<(Vec3, Vec3)> {
    let (end_arc, end_point) = end;
    if end_arc <= from {
        return None;
    }
    let b = if end_arc <= to {
        end_point
    } else {
        plan.line.at(to)
    };
    Some((plan.line.at(from), b))
}

/// Whether the inset camera draws `showing`: only a corner's; the kill cam's lesson draws the
/// stage on the world.
fn inset_active(showing: &super::Showing) -> bool {
    showing.surface == super::Surface::Corner
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
    mut shell: Query<
        (&mut Transform, &mut Visibility),
        (
            With<StageShell>,
            Without<LineSeg>,
            Without<MarkItem>,
            Without<ConeShell>,
            Without<BoxTint>,
            Without<StageCamera>,
        ),
    >,
    mut gizmos: Gizmos<StageGizmos>,
) {
    let Ok((mut camera, mut eye, mut projection)) = camera.single_mut() else {
        return;
    };
    let Ok((mut shell_pose, mut shell_visibility)) = shell.single_mut() else {
        return;
    };
    let Some(showing) = corner.showing else {
        camera.is_active = false;
        shell_visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    camera.is_active = inset_active(&showing);
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

    // The shell, nose at the head, at the record's calibre; the trail ends at its tail.
    let drawn = shell_drawn(plan, kind, c, reduce);
    let end = trail_end(plan, head, drawn);
    if drawn {
        let want = shell_transform(plan, head);
        if *shell_pose != want {
            *shell_pose = want;
        }
        shell_visibility.set_if_neq(Visibility::Inherited);
    } else {
        shell_visibility.set_if_neq(Visibility::Hidden);
    }

    let trail_width = plan.calibre * TRAIL_CALIBRES;
    for (seg, material, mut transform, mut visibility) in &mut lines {
        let Some((a, b)) = trail_span(plan, seg.from, seg.to, end) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let span = b - a;
        let length = span.length();
        if length < 1e-5 {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        *transform = Transform::from_translation(0.5 * (a + b))
            .with_rotation(Quat::from_rotation_arc(Vec3::Y, span / length))
            .with_scale(Vec3::new(trail_width, length, trail_width));
        if let Some(mut m) = glows.get_mut(&material.0) {
            m.set_fade(line_fade);
            m.set_intensity(LINE_GLOW * seg.brightness);
        }
        visibility.set_if_neq(Visibility::Inherited);
    }

    for (mark, material, mut visibility) in &mut marks {
        let fade = fade_from(time_at_arc(plan, kind, mark.at_arc, reduce));
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
    let motion = |rate_deg: f32, from: f32| {
        if settings.reduce_motion {
            0.0
        } else {
            rate_deg.to_radians() * (c - from).max(0.0)
        }
    };
    // The eye stands where the metre figures put it, on `direction` from the `wide` sphere (centre,
    // radius) that frames, and so as clear of the hull as the main gun's view; a small calibre's
    // tighter `tight` sphere is framed by aiming at it and narrowing the field, never by closing
    // the distance. Where the reach leaves the metres in force the two spheres are one and the
    // field is the corner's own.
    let aim = |wide: (Vec3, f32), tight: (Vec3, f32), direction: Vec3, up: Vec3, margin: f32| {
        let eye = wide.0 + direction * (wide.1 * margin / half_fov.sin());
        let half = (tight.1 * margin / eye.distance(tight.0).max(1e-3))
            .clamp(1e-4, half_fov.sin())
            .asin();
        (
            Transform::from_translation(eye).looking_at(tight.0, up),
            Projection::Perspective(PerspectiveProjection {
                fov: 2.0 * half,
                ..default()
            }),
        )
    };
    // A three-quarter view from the shot's side, orbiting about the vertical at `rate` (deg/s).
    let three_quarter = |wide: &[Vec3], tight: &[Vec3], rate: f32, from: f32| {
        let (wide_center, wide_radius) = enclose(wide);
        let (center, radius) = framed(plan, tight);
        let d = plan.line.direction_at(plan.contact_arc);
        let flat = Vec3::new(d.x, 0.0, d.z)
            .try_normalize()
            .unwrap_or(Vec3::NEG_Z);
        let yaw = 55f32.to_radians() + motion(rate, from) + orbit.x;
        let side = Quat::from_rotation_y(yaw) * -flat;
        let pitch = (0.5 + orbit.y).clamp(-1.2, 1.4);
        let direction = (side + Vec3::Y * pitch).normalize();
        aim(
            (wide_center, wide_radius.max(1.0)),
            (center, radius.max(plan.reach(1.0))),
            direction,
            Vec3::Y,
            1.1,
        )
    };
    match kind {
        CornerKind::Glass | CornerKind::Victim => {
            // The whole hull: the record's path inside the tank its components span.
            let mut points = plan.framed_points();
            points.extend(plan.boxes.iter().filter_map(|b| b.center));
            three_quarter(&points, &points, timeline::ORBIT_DEG_S, 0.0)
        }
        CornerKind::Damaged => {
            let incoming = plan.line.direction_at(plan.contact_arc - 1e-3);
            let points = |leg: f32| {
                let mut points = vec![
                    plan.impact_point - incoming * leg,
                    plan.line.at(plan.end_arc.min(plan.contact_arc + leg)),
                    plan.impact_point,
                ];
                points.extend(
                    plan.boxes
                        .iter()
                        .filter(|b| b.dose() > 0.0)
                        .filter_map(|b| b.center),
                );
                points
            };
            three_quarter(
                &points(LEG_FRAME_M),
                &points(plan.reach(LEG_FRAME_M)),
                timeline::DRIFT_DEG_S,
                kind.hold_from(),
            )
        }
        CornerKind::Slice => {
            let (normal, _) = stage.plane.unwrap_or((Vec3::X, Vec3::ZERO));
            let up = (Vec3::Y - normal * normal.y)
                .try_normalize()
                .unwrap_or_else(|| normal.any_orthonormal_vector());
            let yawed = Quat::from_axis_angle(up, orbit.x) * normal;
            let wide = slice_frame(plan, normal, up, SLICE_LEAD_M, SLICE_MIN_HEIGHT_M);
            let tight = slice_frame(
                plan,
                normal,
                up,
                plan.reach(SLICE_LEAD_M),
                plan.reach(SLICE_MIN_HEIGHT_M),
            );
            if settings.slice_ortho {
                (
                    Transform::from_translation(tight.center + yawed * 10.0).looking_to(-yawed, up),
                    Projection::Orthographic(OrthographicProjection {
                        scaling_mode: ScalingMode::AutoMin {
                            min_width: tight.width * 1.1,
                            min_height: tight.height * 1.1,
                        },
                        ..OrthographicProjection::default_3d()
                    }),
                )
            } else {
                // The eye stands as far off the plane as the metre figures put it; the tight frame
                // is held by narrowing the field from there, never by closing the distance.
                let distance = 0.55 * wide.height / half_fov.tan();
                let half = (0.55 * tight.height / distance).atan().min(half_fov);
                (
                    Transform::from_translation(tight.center + yawed * distance)
                        .looking_to(-yawed, up),
                    Projection::Perspective(PerspectiveProjection {
                        fov: 2.0 * half,
                        ..default()
                    }),
                )
            }
        }
        CornerKind::Ricochet => {
            let Some(bounce) = plan.bounce.as_ref() else {
                let points = plan.framed_points();
                return three_quarter(&points, &points, timeline::DRIFT_DEG_S, 0.0);
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
            let points = |leg: f32| {
                [
                    bounce.point - bounce.incoming * leg,
                    bounce.point + bounce.outgoing * leg,
                    bounce.point,
                ]
            };
            let (wide_center, wide_radius) = enclose(&points(LEG_FRAME_M));
            let (center, radius) = framed(plan, &points(plan.reach(LEG_FRAME_M)));
            // The struck face's normal points up the frame: the legs open above the face.
            let up = (bounce.normal - normal * bounce.normal.dot(normal))
                .try_normalize()
                .unwrap_or_else(|| normal.any_orthonormal_vector());
            let drift = motion(timeline::DRIFT_DEG_S, kind.hold_from());
            let direction = Quat::from_axis_angle(up, drift + orbit.x) * normal;
            aim(
                (wide_center, wide_radius.max(0.6)),
                (center, radius.max(plan.reach(0.6))),
                direction,
                up,
                1.15,
            )
        }
    }
}

/// The slice's frame on its plane: centre, and width and height across and up the plane at a 1.6
/// aspect, at most [`FRAME_CALIBRES`] calibres either side of the stop (else the contact).
struct SliceFrame {
    center: Vec3,
    width: f32,
    height: f32,
}

/// The slice frame for a `lead` before the first crossing and past the stop (that shortened by
/// 0.6), at least `min_height` tall across the shot.
fn slice_frame(plan: &Plan, normal: Vec3, up: Vec3, lead: f32, min_height: f32) -> SliceFrame {
    let d = plan.chord_direction();
    let start = plan
        .crossings
        .first()
        .map_or(plan.impact_point, |crossing| crossing.entry)
        - d * lead;
    let end = plan.stop.unwrap_or(plan.line.at(plan.end_arc)) + d * (lead * 0.6);
    let right = (-normal).cross(up);
    let width = (end - start).dot(right).abs();
    let height = ((end - start).dot(up).abs() + 6.0 * plan.calibre)
        .max(min_height)
        .max(width / 1.6);
    let width = width.max(height * 1.6);
    let cap = FRAME_CALIBRES * plan.calibre;
    if 0.5 * width <= cap {
        return SliceFrame {
            center: 0.5 * (start + end),
            width,
            height,
        };
    }
    SliceFrame {
        center: plan.stop.unwrap_or(plan.impact_point),
        width: 2.0 * cap,
        height: 2.0 * cap / 1.6,
    }
}

/// The sphere the corner frames: the one around `points`, or, when that is wider than
/// [`FRAME_CALIBRES`] calibres, one that wide about the stop, else the contact.
fn framed(plan: &Plan, points: &[Vec3]) -> (Vec3, f32) {
    let (center, radius) = enclose(points);
    let cap = FRAME_CALIBRES * plan.calibre;
    if radius <= cap {
        return (center, radius);
    }
    (plan.stop.unwrap_or(plan.impact_point), cap)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage_cam::RoundEnd;

    /// The inset draws a corner showing and stays off for the kill cam's lesson.
    #[test]
    fn the_inset_draws_only_the_corner() {
        let mut showing = super::super::Showing {
            record: 0,
            turn: 0,
            kind: CornerKind::Glass,
            c: 0.5,
            opacity: 1.0,
            scale: 1.0,
            remaining: 1.0,
            surface: super::super::Surface::Corner,
        };
        assert!(inset_active(&showing));
        showing.surface = super::super::Surface::World;
        assert!(!inset_active(&showing));
    }

    /// Tolerance on a drawn position (m): DERIVED, f32 rounding of a transform over a few metres
    /// is under 1e-5 m.
    const POSITION_TOL_M: f32 = 1.0e-4;

    /// The model's bounds in scene 0, every mesh's accessor bounds carried through the transforms of
    /// its node and all its ancestors.
    fn scene_bounds(document: &gltf::Document) -> (Vec3, Vec3) {
        fn walk(node: gltf::Node, parent: Mat4, bounds: &mut (Vec3, Vec3)) {
            let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
            if let Some(mesh) = node.mesh() {
                for primitive in mesh.primitives() {
                    let b = primitive.bounding_box();
                    for corner in 0..8 {
                        let pick = |bit: usize, axis: usize| {
                            if corner & bit == 0 {
                                b.min[axis]
                            } else {
                                b.max[axis]
                            }
                        };
                        let p =
                            world.transform_point3(Vec3::new(pick(1, 0), pick(2, 1), pick(4, 2)));
                        bounds.0 = bounds.0.min(p);
                        bounds.1 = bounds.1.max(p);
                    }
                }
            }
            for child in node.children() {
                walk(child, world, bounds);
            }
        }
        let mut bounds = (Vec3::MAX, Vec3::MIN);
        let scene = document.scenes().next().expect("a scene");
        for node in scene.nodes() {
            walk(node, Mat4::IDENTITY, &mut bounds);
        }
        bounds
    }

    /// Whether the scene's bounds are one model calibre across, its tip at the origin and its body
    /// along +Z for the model length: what the scale law and the nose placement assume.
    fn model_fits((lo, hi): (Vec3, Vec3)) -> bool {
        let size = hi - lo;
        (size.x - SHELL_MODEL_CALIBRE_M).abs() < 1e-3
            && (size.y - SHELL_MODEL_CALIBRE_M).abs() < 1e-3
            && lo.z.abs() < 1e-4
            && (hi.z - SHELL_MODEL_LENGTH_M).abs() < 1e-3
            && (lo.x + hi.x).abs() < 1e-4
            && (lo.y + hi.y).abs() < 1e-4
    }

    /// The shell model, as scene 0 places it (every ancestor's transform included), is one
    /// calibre of [`SHELL_MODEL_CALIBRE_M`] across, its tip at the origin and its body along +Z for
    /// [`SHELL_MODEL_LENGTH_M`]. A mesh node of identity transform under a scaled parent fails it.
    #[test]
    fn the_shell_model_has_its_tip_at_the_origin() {
        let path = crate::assets::asset_root().join(SHELL_SCENE);
        let model = gltf::Gltf::open(&path).unwrap();
        let bounds = scene_bounds(&model.document);
        assert!(model_fits(bounds), "{bounds:?}");

        // The same bounds under a parent scaled 2×: the mesh node itself stays identity.
        let text = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],
            "nodes":[{"scale":[2,2,2],"children":[1]},{"mesh":0}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],
            "buffers":[{"byteLength":36}],"bufferViews":[{"buffer":0,"byteLength":36}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3",
                "min":[-0.044,-0.044,0.0],"max":[0.044,0.044,0.263]}]}"#;
        let scaled = gltf::Gltf::from_slice(text.as_bytes()).unwrap();
        assert!(!model_fits(scene_bounds(&scaled.document)));
    }

    /// For every record: the shell sets off from the drawn span's start as its travel window opens,
    /// crosses the span at one constant speed, and is drawn nose first on the line at the record's
    /// calibre; a round the record has rest keeps its nose on the record's last point to the end of
    /// the frame, and one the record has leave is drawn no longer once its nose runs off the end of
    /// its path, and never under reduce motion.
    #[test]
    fn the_shell_ends_where_the_record_ends() {
        let library = Library::load();
        for record in &library.records {
            let plan = &record.plan;
            let kind = plan.kind;
            let (from, to) = kind.line_window();
            let name = record.stem;
            assert_eq!(head_arc(plan, kind, from, false), plan.start_arc, "{name}");
            assert!(
                (head_arc(plan, kind, to, false) - plan.end_arc).abs() < 1e-3,
                "{name}"
            );
            let samples = 16;
            let step = (to - from) / samples as f32;
            let increments: Vec<f32> = (0..samples)
                .map(|k| {
                    let c = from + step * k as f32;
                    head_arc(plan, kind, c + step, false) - head_arc(plan, kind, c, false)
                })
                .collect();
            for d in &increments {
                assert!((d - increments[0]).abs() < 1e-4, "{name}: {increments:?}");
            }
            for k in 0..=samples {
                let head = head_arc(plan, kind, from + step * k as f32, false);
                let pose = shell_transform(plan, head);
                assert!(
                    pose.translation.distance(plan.line.at(head)) < POSITION_TOL_M,
                    "{name}: the nose is off the line"
                );
                let forward = pose.rotation * Vec3::NEG_Z;
                assert!(
                    forward.dot(plan.line.direction_at(head)) > 1.0 - 1e-5,
                    "{name}: the nose does not lead"
                );
                let across = pose.scale.x * SHELL_MODEL_CALIBRE_M;
                assert!((across - plan.calibre).abs() < 1e-6, "{name}: {across}");
                assert_eq!(pose.scale, Vec3::splat(pose.scale.x), "{name}");
            }
            let last = *plan.line.points.last().unwrap();
            match record.report.end {
                RoundEnd::Left { .. } => {
                    let gone = time_at_arc(plan, kind, plan.line.length(), false) + 1e-3;
                    assert!(!shell_drawn(plan, kind, gone, false), "{name}");
                    assert!(!shell_drawn(plan, kind, to, true), "{name}");
                }
                RoundEnd::Rest { .. } => {
                    for reduce in [false, true] {
                        let end = kind.duration();
                        assert!(shell_drawn(plan, kind, end, reduce), "{name}");
                        let nose = plan.line.at(head_arc(plan, kind, end, reduce));
                        assert!(nose.distance(last) < 1e-4, "{name}");
                    }
                }
            }
            for s in [plan.contact_arc, plan.end_arc] {
                let at = time_at_arc(plan, kind, s, false);
                assert!((head_arc(plan, kind, at, false) - s).abs() < 1e-3, "{name}");
            }
        }
    }

    /// A round the record has leave the tank is never left parked: with its recorded run past the
    /// point it left shorter than the drawn run-out, it is gone once its nose runs off the path,
    /// and never drawn under reduce motion. Each leaving record restated with its path cut 5 cm past
    /// the point it left (fixture data).
    #[test]
    fn a_round_that_left_is_never_parked() {
        let library = Library::load();
        let asset = crate::damage_cam::shapes::TankAsset::load(&library.vehicle).unwrap();
        let mut leaving = 0;
        for record in &library.records {
            let mut report = record.report.clone();
            let RoundEnd::Left { at } = report.end else {
                continue;
            };
            let at = Vec3::from(at);
            let k = report
                .path
                .iter()
                .position(|p| Vec3::from(p.point).distance(at) < 1e-5)
                .unwrap();
            let Some(next) = report.path.get(k + 1).copied() else {
                continue;
            };
            let leg = Vec3::from(next.point) - at;
            report.path.truncate(k + 1);
            report.path.push(crate::damage_cam::PathPoint {
                t: report.path[k].t + (next.t - report.path[k].t) * 0.05 / leg.length(),
                point: (at + leg.normalize() * 0.05).to_array(),
            });
            let plan = Plan::new(&report, &asset.shapes(&report.servo_deg));
            let kind = plan.kind;
            let end = kind.duration();
            assert!(!shell_drawn(&plan, kind, end, false), "{}", record.stem);
            assert!(!shell_drawn(&plan, kind, end, true), "{}", record.stem);
            leaving += 1;
        }
        assert!(leaving > 0, "a record leaves the tank");
    }

    /// The trail meets the shell's rendered tail at every sample, across a bend too: the trail leg
    /// holding the trail's end ends at the model's tail, carried by the shell's own transform, and
    /// the drawn legs join end to end. Sampled at every record's bounce with the nose half a model
    /// length past it.
    #[test]
    fn the_trail_meets_the_shells_tail_across_a_bend() {
        let library = Library::load();
        let mut bends = 0;
        for record in &library.records {
            let plan = &record.plan;
            let Some(bounce) = &plan.bounce else {
                continue;
            };
            let length = SHELL_MODEL_LENGTH_M * plan.calibre / SHELL_MODEL_CALIBRE_M;
            let head = bounce.arc + 0.5 * length;
            let pose = shell_transform(plan, head);
            let tail = pose.translation - pose.rotation * Vec3::NEG_Z * length;
            let end = trail_end(plan, head, true);
            let line = &plan.line;
            let spans: Vec<(Vec3, Vec3)> = (1..line.points.len())
                .filter_map(|i| {
                    let (from, to) = (line.arc[i - 1].max(plan.start_arc), line.arc[i]);
                    (to > from)
                        .then(|| trail_span(plan, from, to, end))
                        .flatten()
                })
                .collect();
            let (_, trail_tip) = *spans.last().expect("a trail is drawn");
            assert!(
                trail_tip.distance(tail) < POSITION_TOL_M,
                "{}: the trail ends {} m from the shell's tail",
                record.stem,
                trail_tip.distance(tail)
            );
            for pair in spans.windows(2) {
                assert!(
                    pair[0].1.distance(pair[1].0) < POSITION_TOL_M,
                    "{}",
                    record.stem
                );
            }
            bends += 1;
        }
        assert!(bends > 0, "a record bounces");
    }

    /// The slice frames a small round from where the metre figures put the eye, narrowing the
    /// field rather than closing the distance, and never frames wider than [`FRAME_CALIBRES`]
    /// calibres either side of the stop: every slice record, as recorded and with its calibre cut
    /// to 2 mm (fixture data).
    #[test]
    fn the_slice_narrows_the_field_and_holds_the_calibre_cap() {
        let library = Library::load();
        let asset = crate::damage_cam::shapes::TankAsset::load(&library.vehicle).unwrap();
        let mut slices = 0;
        for record in &library.records {
            if record.plan.kind != CornerKind::Slice {
                continue;
            }
            for calibre_mm in [record.report.projectile.caliber_mm, 2.0] {
                let mut report = record.report.clone();
                report.projectile.caliber_mm = calibre_mm;
                let plan = Plan::new(&report, &asset.shapes(&report.servo_deg));
                let stage = Stage {
                    plane: Some(slice_plane(&plan)),
                    ..default()
                };
                let (normal, _) = stage.plane.unwrap();
                let up = (Vec3::Y - normal * normal.y).normalize();
                let wide = slice_frame(&plan, normal, up, SLICE_LEAD_M, SLICE_MIN_HEIGHT_M);
                let wide_distance = 0.55 * wide.height / (0.5 * CORNER_FOV_DEG.to_radians()).tan();
                let cap = 2.0 * FRAME_CALIBRES * plan.calibre * 1.1 + 1e-5;
                let perspective = Settings {
                    slice_ortho: false,
                    ..Settings::default()
                };
                let (eye, projection) = framing(
                    &plan,
                    CornerKind::Slice,
                    0.0,
                    &perspective,
                    &stage,
                    Vec2::ZERO,
                );
                let off_plane = (eye.translation - wide.center).dot(normal);
                assert!(
                    (off_plane - wide_distance).abs() < 1e-3,
                    "{} at {calibre_mm} mm: the eye stands {off_plane} m off, not {wide_distance}",
                    record.stem
                );
                let Projection::Perspective(lens) = projection else {
                    panic!("perspective");
                };
                assert!(lens.fov <= CORNER_FOV_DEG.to_radians() + 1e-6);
                let seen = 2.0 * off_plane * (0.5 * lens.fov).tan();
                assert!(seen <= cap / 1.6 + 1e-3, "{}: {seen} m tall", record.stem);
                let (_, projection) = framing(
                    &plan,
                    CornerKind::Slice,
                    0.0,
                    &Settings::default(),
                    &stage,
                    Vec2::ZERO,
                );
                let Projection::Orthographic(ortho) = projection else {
                    panic!("orthographic");
                };
                let ScalingMode::AutoMin { min_width, .. } = ortho.scaling_mode else {
                    panic!("auto min");
                };
                assert!(min_width <= cap, "{}: {min_width} m wide", record.stem);
            }
            slices += 1;
        }
        assert!(slices > 0, "a record opens on the slice");
    }

    /// Each of a tank's meshes belongs to the component its nearest named node of the plan's
    /// names is: a component nested in another is its own, a component of several meshes owns
    /// each, a mesh under no component belongs to none. The names are the first record's own.
    #[test]
    fn each_mesh_belongs_to_its_nearest_component() {
        let library = Library::load();
        let plan = &library.records[0].plan;
        let (outer, inner) = (plan.boxes[0].name.clone(), plan.boxes[1].name.clone());
        let none = "a node no component names".to_owned();
        let e = |i: u32| Entity::from_raw_u32(i).unwrap();
        let chains: Vec<(Entity, Vec<String>)> = vec![
            (e(1), vec![outer.clone()]),
            (e(2), vec![none.clone(), outer.clone()]),
            (e(3), vec![inner.clone(), outer.clone()]),
            (
                e(4),
                vec![none.clone(), inner.clone(), none.clone(), outer.clone()],
            ),
            (e(5), vec![none.clone()]),
        ];
        let found = component_meshes(
            plan,
            chains
                .iter()
                .map(|(entity, names)| (*entity, names.as_slice(), Handle::default())),
        );
        let got: Vec<(Entity, usize)> = found.iter().map(|m| (m.entity, m.index)).collect();
        assert_eq!(got, [(e(1), 0), (e(2), 0), (e(3), 1), (e(4), 1)]);
    }
}
