//! The corner's stage: the struck tank's stage view dressed for the frame, and the record drawn
//! over it — the round as the game's shell model at the record's calibre, travelling the recorded
//! path nose first with a thin trail behind it, a ring where it crossed each plate or glanced off a
//! face, every spall cone in a cool hue cut where its directions stopped ([`look::field`], nested
//! reach surfaces cut by the plane in the slice), and each struck component's real mesh on
//! the state scale ([`look::component`]):
//! filled red from the bottom up to the share of its hit points lost, a dead one charcoal and
//! hatched.
//! The shell rests where the record ends; a round the record has leave the tank runs on out of the
//! frame. A cone's front runs from its origin at the shell's own on-screen speed.
//! Rebuilt when the corner's record or frame changes; animated every frame from the corner's clock
//! alone, so a scrub lands on the same picture.
//!
//! Frames: the see-through hull (the base look, [`look`], in the corner and in the kill cam's
//! lesson alike), the plate slice (the tank's own materials, everything on the eye's side of a
//! vertical plane through the chord dissolved away), and the outside view (the tank as built).
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

use super::field::{self, reach_mesh};
use super::look::{self, FieldPrint, LookMaterial, MaskLayer, Masks};
use super::material::{GlowMaterial, OutlineMaterial, SliceCut, SliceMaterial, SliceParams};
use super::plan::{BoxPlan, Cone, Plan, Reach};
use super::timeline::{self, CornerKind};
use super::view::{Proxy, StageView, ViewMesh, ViewRoot};
use super::{Corner, CornerRig, Exhibits, Knobs};
use crate::render_policy::{self, CameraProfile, LightProfile, VisualScope};

/// The corner's clear colour.
const BACKGROUND: Color = Color::srgb_u8(0x0F, 0x13, 0x18);
/// The trail's colour.
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
/// The slice's and the ricochet's framed radius is at most this many calibres (TUNED), about the
/// stop or else the contact: 7.0 m for 88 mm, 0.63 m for a 7.9 mm round (DERIVED).
const FRAME_CALIBRES: f32 = 80.0;
/// A ring at a crossing: a calibre across, this thick (fraction of its radius). TUNED, as is its
/// glow.
const RING_THICKNESS: f32 = 0.12;
const RING_GLOW: f32 = 2.0;
/// In the slice, the spall cone is drawn as this many nested copies of its reach surface, scaled
/// about its origin and cut by the plane, so its density reads as depth; the on-axis brightness of all of them together is
/// [`CONE_PEAK`] at the top of the dose scale. TUNED, as are the cone's and the boxes' figures
/// below.
const CONE_SHELLS: usize = 4;
const CONE_PEAK: f32 = 0.35;
/// The shells' emission gain over [`CONE_PEAK`], for the bloom: a diagram choice.
const CONE_GAIN: f32 = 4.0;
/// The outer reach surface's view-angle glow at the top of the dose scale, held at this fraction
/// of the nested shells' on-axis sum so the directions' own strengths, not the outline, carry the
/// field's shape.
const CONE_RIM_SHARE: f32 = 0.3;
/// The slice's field density falls as `(r₀ / r)²` from its origin, unity inside `r₀`, this
/// fraction of the cone's longest reach. Every other frame draws the law's own `1/(r² + ε²)`
/// ([`look::field`]), whose entries and exits cancel only across the whole volume, so it takes no
/// plane cut.
const FIELD_FALLOFF_REACH: f32 = 0.3;
/// The slice's cut face.
const CUT: Color = Color::srgb_u8(0xC8, 0xCD, 0xD3);
/// The corner camera's vertical field (deg) in perspective.
const CORNER_FOV_DEG: f32 = 30.0;
/// The outside view frames this much of each leg either side of the contact (m). This and every
/// other framing length below is taken through [`Plan::reach`], so a small calibre frames tighter.
/// TUNED, as are the slice's figures below.
const LEG_FRAME_M: f32 = 1.5;
/// The Pen and Damaged frames look along the shot turned this far (deg) off it. TUNED.
const SHOT_NUDGE_DEG: f32 = 20.0;
/// Those frames hold the tank's bounds' sphere this much inside the field. TUNED.
const BOUNDS_MARGIN: f32 = 1.1;
/// A direction whose cross product with the vertical is shorter than this (the sine of 0.06°) is
/// vertical, for choosing an axis or an up.
const VERTICAL_SIN: f32 = 1.0e-3;
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

/// One copy of cone `cone`'s reach surface, scaled about its origin; `rim` marks the outer surface.
#[derive(Component)]
pub(super) struct ConeShell {
    cone: usize,
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
}

/// The built stage's shared state.
#[derive(Resource, Default)]
pub struct Stage {
    key: Option<StageKey>,
    /// Whether the built stage holds materials made before a change they depend on (the masks).
    stale: bool,
    /// The slice plane: unit normal toward the eye, and a point on it.
    plane: Option<(Vec3, Vec3)>,
    slices: HashMap<AssetId<StandardMaterial>, Handle<SliceMaterial>>,
    camera: Option<Entity>,
    /// The hull the built stage is drawn under: its frame is the record's, its up the tank's.
    hull: Option<Entity>,
}

impl Stage {
    /// Rebuild at the next [`rebuild`].
    pub(super) fn invalidate(&mut self) {
        self.stale = true;
    }
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
    mut stage: ResMut<Stage>,
) {
    spawn_inset(&mut commands, &asset_server, &mut images, &mut stage);
}

/// The inset camera, inactive, into a placeholder image until the corner's layout sizes one; the
/// stage's two key lights; the shell, hidden.
pub fn spawn_inset(
    commands: &mut Commands,
    asset_server: &AssetServer,
    images: &mut Assets<Image>,
    stage: &mut Stage,
) {
    let placeholder = images.add(Image::new_target_texture(
        16,
        10,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    let camera = commands.spawn((
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
        crate::render_scale::NativeResolution,
        CornerRig,
    ));
    stage.camera = Some(camera.id());
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
            CornerRig,
        ));
    }
    let shell: Handle<WorldAsset> =
        asset_server.load(GltfAssetLabel::Scene(0).from_asset(SHELL_SCENE));
    commands.spawn((
        WorldAssetRoot(shell),
        Transform::IDENTITY,
        Visibility::Hidden,
        VisualScope::PEN_CAM_STAGE,
        StageShell,
        CornerRig,
    ));
}

/// Rebuild the stage when the corner's record or frame changes.
pub fn rebuild(
    mut commands: Commands,
    corner: Res<Corner>,
    exhibits: Res<Exhibits>,
    mut stage: ResMut<Stage>,
    items: Query<Entity, With<StageItem>>,
    view: Res<StageView>,
    tank: Query<(Entity, &ViewMesh, &Mesh3d), Without<Proxy>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut glows: ResMut<Assets<GlowMaterial>>,
    mut outlines: ResMut<Assets<OutlineMaterial>>,
    standard: Res<Assets<StandardMaterial>>,
    mut slices: ResMut<Assets<SliceMaterial>>,
    mut looks: ResMut<Assets<LookMaterial>>,
    masks: Res<Masks>,
) {
    let key = corner.showing.map(|showing| StageKey {
        turn: showing.turn,
        record: showing.record,
        kind: showing.kind,
    });
    if key == stage.key && !stage.stale {
        return;
    }
    let Some(hull) = view.hull else {
        return;
    };
    stage.key = key.clone();
    stage.stale = false;
    stage.hull = Some(hull);
    for entity in &items {
        commands.entity(entity).despawn();
    }
    // Every frame dresses the stage's meshes afresh; with none on show they hold no frame's
    // materials, and so none of the masks.
    for (entity, mesh, _) in &tank {
        if mesh.root == ViewRoot::Stage {
            commands.entity(entity).remove::<(
                MeshMaterial3d<StandardMaterial>,
                MeshMaterial3d<GlowMaterial>,
                MeshMaterial3d<SliceMaterial>,
                MeshMaterial3d<LookMaterial>,
            )>();
        }
    }
    let Some(key) = key else {
        return;
    };
    let Some(exhibit) = exhibits.0.get(key.record) else {
        return;
    };
    let plan = &*exhibit.plan;
    let kind = key.kind;
    stage.plane = (kind == CornerKind::Slice).then(|| slice_plane(plan));
    let mut tier_materials: HashMap<super::view::Tier, (Handle<LookMaterial>, MaskLayer)> =
        HashMap::new();
    let hull_mask = looks.add(look::hull_mask(&masks));

    // The tank, dressed for the frame.
    for (entity, mesh, handle) in &tank {
        if mesh.root != ViewRoot::Stage {
            continue;
        }
        let mut e = commands.entity(entity);
        match kind {
            CornerKind::Glass | CornerKind::Victim => {
                let entry = tier_materials.entry(mesh.tier).or_insert_with(|| {
                    let draw = look::hull(mesh.tier, &masks);
                    (looks.add(draw.material), draw.mask)
                });
                let (material, mask) = (entry.0.clone(), entry.1);
                e.insert((MeshMaterial3d(material), Visibility::Inherited));
                commands.spawn((
                    Mesh3d(handle.0.clone()),
                    MeshMaterial3d(hull_mask.clone()),
                    Transform::IDENTITY,
                    match mask {
                        MaskLayer::Hull => VisualScope::PEN_CAM_HULL_MASK,
                        MaskLayer::Part => VisualScope::PEN_CAM_PART_MASK,
                    },
                    StageItem,
                    ChildOf(entity),
                ));
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
        &mut outlines,
        plan,
        kind,
        hull,
        stage.plane,
        &components,
        LookCtx {
            looks: &mut looks,
            masks: &masks,
        },
    );
}

/// What drawing a record in the base look needs.
pub struct LookCtx<'a> {
    pub looks: &'a mut Assets<LookMaterial>,
    pub masks: &'a Masks,
}

/// Draw `plan`'s record over a tank in frame `kind`: the trail, the rings, the spall cones and the
/// components on the state scale. Every element but the tints is spawned under `hull`, an entity
/// whose frame is the record's (the struck hull's); each tint is a copy of one of `components`, the
/// tank's own meshes of the components the record names, spawned under that mesh. `clip` is the
/// slice plane (unit normal toward the eye, a point on it) the cones are cut by. Everything spawned
/// is a [`StageItem`], hidden until [`animate`] places it.
#[allow(clippy::too_many_arguments)]
pub fn spawn_glass(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    glows: &mut Assets<GlowMaterial>,
    outlines: &mut Assets<OutlineMaterial>,
    plan: &Plan,
    kind: CornerKind,
    hull: Entity,
    clip: Option<(Vec3, Vec3)>,
    components: &[ComponentMesh],
    look_ctx: LookCtx,
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
    // The Ricochet frame's eye turns about the struck face's normal, square to the plane of both
    // legs ([`framing`]); a bounce leaves in its plane of incidence, which holds that normal, so
    // the eye lies in the face's plane and a gouge on the face would read as a flat bar at the apex.
    if let Some(bounce) = &plan.bounce
        && kind != CornerKind::Ricochet
    {
        mark(
            commands,
            ring_on_face(bounce.point, bounce.normal, bounce.incoming, radius),
            bounce.arc,
            RING_GLOW,
        );
    }

    // Spall cones: the law's field, or in the slice nested reach surfaces and a muted outer one,
    // clipped by the slice plane.
    for (index, cone) in plan.cones.iter().enumerate() {
        if matches!(kind, CornerKind::Ricochet) {
            break;
        }
        let mesh = meshes.add(reach_mesh(cone));
        // The field emerges through the round's own exit hole.
        let styled = (kind != CornerKind::Slice).then(|| {
            look::field(
                0.5 * plan.calibre,
                cone.strength,
                field::profile(cone),
                look_ctx.masks,
            )
        });
        if let Some(mut material) = styled {
            let mut e = commands.spawn((
                Mesh3d(mesh.clone()),
                Transform::from_translation(cone.origin),
                Visibility::Hidden,
                ConeShell {
                    cone: index,
                    rim: true,
                    peak: 0.0,
                },
                scope,
                StageItem,
                ChildOf(hull),
            ));
            material.set_fade(0.0);
            e.insert(MeshMaterial3d(look_ctx.looks.add(material)));
            continue;
        }
        for shell in 0..=CONE_SHELLS {
            let rim = shell == CONE_SHELLS;
            let k = if rim {
                1.0
            } else {
                (shell + 1) as f32 / CONE_SHELLS as f32
            };
            let (mut material, peak) = field_material(cone, rim);
            material.set_clip(clip);
            material.set_fade(0.0);
            commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(glows.add(material)),
                Transform::from_translation(cone.origin).with_scale(Vec3::splat(k)),
                Visibility::Hidden,
                ConeShell {
                    cone: index,
                    rim,
                    peak,
                },
                scope,
                StageItem,
                ChildOf(hull),
            ));
        }
    }

    // Components, in their own meshes, on the state scale.
    for component in components {
        let index = component.index;
        let b = &plan.boxes[index];
        let Some(mut fill) = look::component(b, kind, look_ctx.masks) else {
            continue;
        };
        use bevy::camera::primitives::MeshAabb;
        let center = meshes
            .get(&component.mesh)
            .and_then(|mesh| mesh.compute_aabb())
            .map_or(Vec3::ZERO, |aabb| Vec3::from(aabb.center));
        let mut outline =
            OutlineMaterial::new(timeline::STATE_RED.into(), look::STATE_OUTLINE_PX, center);
        fill.set_fade(0.0);
        outline.set_fade(0.0);
        let tint = || {
            (
                Mesh3d(component.mesh.clone()),
                Transform::IDENTITY,
                Visibility::Hidden,
                BoxTint { index },
                scope,
                StageItem,
                ChildOf(component.entity),
            )
        };
        commands.spawn((tint(), MeshMaterial3d(look_ctx.looks.add(fill))));
        commands.spawn((tint(), MeshMaterial3d(outlines.add(outline))));
    }
}

/// One of a cone's reach surfaces, and its peak intensity at full fade: the nested shells carry
/// each direction's strength in their vertex alpha, so the field's angular profile is theirs; the
/// outer shell (`rim`) adds only a view-angle edge at [`CONE_RIM_SHARE`] of their on-axis sum. Both
/// fall off as 1/r² from the origin, in [`timeline::FIELD`]'s cool hue.
fn field_material(cone: &Cone, rim: bool) -> (GlowMaterial, f32) {
    let reach = cone.max_reach();
    let r0 = (FIELD_FALLOFF_REACH * reach).max(1e-3);
    let interior = CONE_GAIN * CONE_PEAK * timeline::field_level(cone.strength);
    let (material, peak) = if rim {
        let peak = CONE_RIM_SHARE * interior;
        (
            GlowMaterial::additive(timeline::FIELD_EDGE.into(), 0.0).with_rim(0.0, peak, 2.5),
            peak,
        )
    } else {
        let peak = interior / CONE_SHELLS as f32;
        (
            GlowMaterial::additive(timeline::FIELD.into(), peak).with_vertex_color(),
            peak,
        )
    };
    (material.with_falloff(r0), peak)
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

/// The corner time at which cone `index` is thrown: the shell's nose at its origin.
fn cone_start(plan: &Plan, kind: CornerKind, index: usize, reduce: bool) -> f32 {
    time_at_arc(plan, kind, plan.cones[index].arc, reduce)
}

/// How far (m) cone `index`'s front has run from its origin at corner time `c`, negative before the
/// cone is thrown. It runs from the moment the shell's nose reaches the origin at the shell's own
/// on-screen speed, [`travel_speed`]: on the shell's replay clock every leg, the one leaving the
/// plate that threw the cone included, plays at that one speed, so this is the round's exit speed,
/// the same in every direction (the record holds no fragment speed). The front keeps pace with
/// the shell and never outruns it.
fn front_radius(plan: &Plan, kind: CornerKind, index: usize, c: f32) -> f32 {
    travel_speed(plan, kind) * (c - cone_start(plan, kind, index, false))
}

/// The corner time at which component `b`'s fill starts: the line's entry, or the moment the dosing
/// cone's front has run the distance the plan gives; the victim's own glass shows its state from
/// the open.
fn box_start(plan: &Plan, kind: CornerKind, b: &BoxPlan, reduce: bool) -> f32 {
    match (kind, b.reached_at) {
        (CornerKind::Victim, _) => 0.0,
        (_, Some(Reach::Line(arc))) => time_at_arc(plan, kind, arc, reduce),
        (_, Some(Reach::Cone { cone, distance })) => {
            if reduce {
                cone_start(plan, kind, cone, true)
            } else {
                cone_start(plan, kind, cone, false) + distance / travel_speed(plan, kind)
            }
        }
        (_, None) => f32::INFINITY,
    }
}

/// The corner time by which every element of `plan` has arrived in frame `kind`: each cone's front
/// at its longest reach and each dosed component's fill complete.
#[cfg(test)]
pub(super) fn settled(plan: &Plan, kind: CornerKind) -> f32 {
    let speed = travel_speed(plan, kind);
    let cones = plan.cones.iter().enumerate().map(|(index, cone)| {
        let reach = cone.max_reach();
        cone_start(plan, kind, index, false) + reach / speed
    });
    let boxes = plan
        .boxes
        .iter()
        .map(|b| box_start(plan, kind, b, false) + timeline::BOX_FILL_S)
        .filter(|t| t.is_finite());
    cones.chain(boxes).fold(kind.line_window().1, f32::max)
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

/// The shell model's transform with its nose at arc `head`: the tip on the line, the body along
/// the chord to the line's point one model length back, scaled from the model's calibre to the
/// record's. On a straight leg the chord is the travel; within a model length past a bend the body
/// pivots across it, so past a bounce it stays on the face's outer side as the path does, where
/// the travel's own direction would stand it inside the face.
fn shell_transform(plan: &Plan, head: f32) -> Transform {
    let nose = plan.line.at(head);
    let back = plan.line.at(head - shell_length(plan));
    let body = (nose - back)
        .try_normalize()
        .unwrap_or_else(|| plan.line.direction_at(head));
    Transform::from_translation(nose)
        .looking_to(body, Vec3::Y)
        .with_scale(Vec3::splat(plan.calibre / SHELL_MODEL_CALIBRE_M))
}

/// The shell model's length (m) at the record's calibre.
fn shell_length(plan: &Plan) -> f32 {
    SHELL_MODEL_LENGTH_M * plan.calibre / SHELL_MODEL_CALIBRE_M
}

/// Where the trail ends: its arc along the line and its point. Behind a drawn shell it ends at the
/// model's tail, which within a model length past a bend stands just off the line, the body's chord
/// across the bend being shorter than the model; otherwise at the nose.
fn trail_end(plan: &Plan, head: f32, drawn: bool) -> (f32, Vec3) {
    if !drawn {
        return (head, plan.line.at(head));
    }
    let tail = shell_transform(plan, head).transform_point(Vec3::Z * SHELL_MODEL_LENGTH_M);
    (head - shell_length(plan), tail)
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
    knobs: Res<Knobs>,
    exhibits: Res<Exhibits>,
    stage: Res<Stage>,
    hulls: Query<&GlobalTransform>,
    mut glows: ResMut<Assets<GlowMaterial>>,
    mut outlines: ResMut<Assets<OutlineMaterial>>,
    mut slices: ResMut<Assets<SliceMaterial>>,
    mut looks: ResMut<Assets<LookMaterial>>,
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
            Option<&MeshMaterial3d<GlowMaterial>>,
            Option<&MeshMaterial3d<LookMaterial>>,
            &mut Visibility,
        ),
        (Without<LineSeg>, Without<MarkItem>, Without<BoxTint>),
    >,
    mut boxes: Query<
        (
            &BoxTint,
            Option<&MeshMaterial3d<OutlineMaterial>>,
            Option<&MeshMaterial3d<LookMaterial>>,
            &mut Visibility,
        ),
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
    let Some(exhibit) = exhibits.0.get(showing.record) else {
        camera.is_active = false;
        shell_visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    camera.is_active = inset_active(&showing);
    let plan = &*exhibit.plan;
    let kind = showing.kind;
    let c = showing.c;
    let reduce = knobs.reduce_motion;
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

    for (shell, material, look_material, mut visibility) in &mut shells {
        // Reduce motion fades the whole field in; otherwise its front runs out from the origin.
        let (front, fade) = if reduce {
            (None, fade_from(cone_start(plan, kind, shell.cone, true)))
        } else {
            let front = front_radius(plan, kind, shell.cone, c);
            (Some(front), if front > 0.0 { 1.0 } else { 0.0 })
        };
        if fade <= 0.0 {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        if let Some(mut m) = material.and_then(|m| glows.get_mut(&m.0)) {
            m.set_fade(fade);
            m.set_front(front);
            if !shell.rim {
                m.set_intensity(shell.peak);
            }
        }
        if let Some(mut m) = look_material.and_then(|m| looks.get_mut(&m.0)) {
            m.set_fade(fade);
            m.set_front(front);
        }
        visibility.set_if_neq(Visibility::Inherited);
    }

    // The state fills rise along the hull's up, the tank's.
    let hull = stage.hull.and_then(|hull| hulls.get(hull).ok());
    // Each field's footprint, in world space, where the base look marks it.
    if let Some(hull) = hull
        && matches!(
            kind,
            CornerKind::Glass | CornerKind::Victim | CornerKind::Damaged
        )
    {
        let linear = hull.affine().matrix3;
        let prints: Vec<FieldPrint> = plan
            .cones
            .iter()
            .enumerate()
            .map(|(index, cone)| {
                let axis = linear * cone.axis;
                let scale = axis.length();
                let reach = cone.max_reach();
                let front = if reduce {
                    if c >= cone_start(plan, kind, index, true) {
                        reach
                    } else {
                        -1.0
                    }
                } else {
                    front_radius(plan, kind, index, c)
                };
                FieldPrint {
                    origin: hull.transform_point(cone.origin),
                    axis: axis / scale.max(1e-6),
                    half_angle: cone.half_angle,
                    reach: reach * scale,
                    front: front * scale,
                    level: timeline::field_level(cone.strength),
                    rays: cone
                        .rays
                        .iter()
                        .map(|ray| {
                            let direction = linear * ray.direction;
                            (direction / direction.length().max(1e-6), ray.reach * scale)
                        })
                        .collect(),
                }
            })
            .collect();
        look::set_footprints(&mut looks, &prints);
    }
    for (tint, outline, look_material, mut visibility) in &mut boxes {
        let b = &plan.boxes[tint.index];
        let start = box_start(plan, kind, b, reduce);
        let fill = if reduce {
            fade_from(start)
        } else {
            timeline::ease_enter(timeline::progress(c, start, timeline::BOX_FILL_S))
        };
        if let Some(mut m) = outline.and_then(|m| outlines.get_mut(&m.0)) {
            m.set_fade(fill);
        }
        if let Some(mut m) = look_material.and_then(|m| looks.get_mut(&m.0)) {
            m.set_fade(fill);
            if let Some(hull) = hull {
                let up = *hull.up();
                let height = |y: f32| hull.transform_point(Vec3::Y * y).dot(up);
                m.set_fill_span(up, b.heights.map(|(lo, hi)| (height(lo), height(hi))));
            }
        }
        visibility.set_if_neq(if fill > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }

    if knobs.axis_line {
        for (index, cone) in plan.cones.iter().enumerate() {
            if c >= cone_start(plan, kind, index, reduce) {
                let reach = cone.max_reach();
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

    let (want, want_projection) = framing(plan, kind, c, &knobs, &stage);
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
    knobs: &Knobs,
    stage: &Stage,
) -> (Transform, Projection) {
    let orbit = knobs.orbit;
    let half_fov = 0.5 * CORNER_FOV_DEG.to_radians();
    let motion = |rate_deg: f32, from: f32| {
        if knobs.reduce_motion {
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
    match kind {
        CornerKind::Glass | CornerKind::Victim => {
            along_shot(plan, orbit, motion(timeline::ORBIT_DEG_S, 0.0))
        }
        CornerKind::Damaged => {
            along_shot(plan, orbit, motion(timeline::DRIFT_DEG_S, kind.hold_from()))
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
            if knobs.slice_ortho {
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
                return along_shot(plan, orbit, motion(timeline::DRIFT_DEG_S, 0.0));
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

/// The Pen and Damaged frames' camera: the eye looks at the impact from the shooter's side, along
/// the shot's own direction at first contact turned [`SHOT_NUDGE_DEG`] off it, so the path reads as
/// a line rather than a dot, and frames the whole tank's bounds whatever the calibre or the hit's
/// extent. The line along the shot is clear up to the first impact, because the round travelled
/// it; the eye's sightline to the impact lies the nudge off that line.
///
/// The nudge turns about `Y × shot`, lifting the eye toward the vertical. A vertical shot has no
/// such axis and turns about the record frame's longitudinal axis (Z) instead, so its eye stands
/// above the tank to one side. The camera stays upright: its up is the world vertical, except when
/// it looks straight up or down, where it is the shot's horizontal heading, so the path runs up the
/// frame.
/// `orbit` (the viewer's drag, rad) yaws the sightline about the vertical and adds to the nudge;
/// from there the eye turns `spin` (rad) about the vertical through the bounds' centre, keeping
/// its distance and height, so the frame's slow orbit starts on the shot line.
///
/// The bounds' sphere fits the vertical field, which binds while the corner is landscape.
fn along_shot(plan: &Plan, orbit: Vec2, spin: f32) -> (Transform, Projection) {
    let half_fov = 0.5 * CORNER_FOV_DEG.to_radians();
    let (center, radius) = match plan.tank_bounds {
        Some((lo, hi)) => (0.5 * (lo + hi), 0.5 * (hi - lo).length()),
        None => enclose(&plan.framed_points()),
    };
    let shot = plan.line.direction_at(plan.contact_arc - 1e-3);
    let lift = Vec3::Y.cross(shot);
    let axis = if lift.length() > VERTICAL_SIN {
        lift.normalize()
    } else {
        Vec3::Z
    };
    let toward_eye = Quat::from_rotation_y(orbit.x)
        * Quat::from_axis_angle(axis, SHOT_NUDGE_DEG.to_radians() + orbit.y)
        * -shot;
    // A vertical shot under a vertical sightline (a drag that undoes the nudge) has no heading;
    // the nudge axis, horizontal, is its up.
    let up = if Vec3::Y.cross(toward_eye).length() > VERTICAL_SIN {
        Vec3::Y
    } else {
        Vec3::new(shot.x, 0.0, shot.z)
            .try_normalize()
            .unwrap_or(axis)
    };
    // The eye on the sightline from the impact, `far` from the bounds' centre: the positive root of
    // `|q + e·t| = far` for `q` the impact off the centre, real while `|q| ≤ far`, which holds
    // for an impact inside the bounds (`|q| ≤ radius < far`).
    let far = radius * BOUNDS_MARGIN / half_fov.sin();
    let q = plan.impact_point - center;
    let b = toward_eye.dot(q);
    let t = -b + (b * b - q.length_squared() + far * far).max(0.0).sqrt();
    let turn = Quat::from_rotation_y(spin);
    let eye = center + turn * (plan.impact_point + toward_eye * t - center);
    (
        Transform::from_translation(eye).looking_at(center, turn * up),
        Projection::Perspective(PerspectiveProjection {
            fov: CORNER_FOV_DEG.to_radians(),
            ..default()
        }),
    )
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
    use super::super::demo::Library;
    use super::super::material::Blend;
    use super::super::plan::LEG_EPS_M;
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
                // Within a model length past a bend the body pivots across it, nose still ahead.
                let line = &plan.line;
                let straight = line.leg_after((head - shell_length(plan)).max(0.0))
                    == line.leg_after(head - 2.0 * LEG_EPS_M);
                let lead = if straight { 1.0 - 1e-5 } else { 0.0 };
                assert!(
                    forward.dot(line.direction_at(head)) > lead,
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

    /// Past a bounce the shell stays on the struck face's outer side, as the record's path does:
    /// for every record that bounces, the model's nose and tail stand no deeper than
    /// [`POSITION_TOL_M`] behind the face's plane at every sample across the first two model
    /// lengths past the apex.
    #[test]
    fn the_shell_never_sinks_into_the_face_it_glances_off() {
        let library = Library::load();
        let mut bounces = 0;
        for record in &library.records {
            let plan = &record.plan;
            let Some(bounce) = &plan.bounce else {
                continue;
            };
            let length = shell_length(plan);
            let samples = 32;
            for k in 0..=samples {
                let head = bounce.arc + 2.0 * length * k as f32 / samples as f32;
                let pose = shell_transform(plan, head);
                let tail = pose.transform_point(Vec3::Z * SHELL_MODEL_LENGTH_M);
                for (part, p) in [("nose", pose.translation), ("tail", tail)] {
                    let depth = (bounce.point - p).dot(bounce.normal);
                    assert!(
                        depth < POSITION_TOL_M,
                        "{}: the {part} stands {depth} m inside the face {} m past the apex",
                        record.stem,
                        head - bounce.arc
                    );
                }
            }
            bounces += 1;
        }
        assert!(bounces > 0, "a record bounces");
    }

    /// No ring the Ricochet frame draws is seen within [`EDGE_ON_DEG`] of edge-on at any moment of
    /// the frame, where it would read as a flat bar rather than a ring.
    #[test]
    fn the_ricochet_frame_sees_no_ring_edge_on() {
        const EDGE_ON_DEG: f32 = 10.0;
        let library = Library::load();
        let mut frames = 0;
        for (index, record) in library.records.iter().enumerate() {
            let plan = &record.plan;
            if plan.kind != CornerKind::Ricochet {
                continue;
            }
            let (mut app, _) = staged(index, CornerKind::Ricochet, Vec3::ZERO);
            let rings: Vec<Transform> = app
                .world_mut()
                .query_filtered::<&Transform, With<MarkItem>>()
                .iter(app.world())
                .copied()
                .collect();
            let samples = 16;
            for k in 0..=samples {
                let c = CornerKind::Ricochet.duration() * k as f32 / samples as f32;
                let (eye, _) = framing(
                    plan,
                    CornerKind::Ricochet,
                    c,
                    &Knobs::default(),
                    &Stage::default(),
                );
                for ring in &rings {
                    let sight = (ring.translation - eye.translation).normalize();
                    let axis = ring.rotation * Vec3::Y;
                    let off_edge = sight.dot(axis).abs().asin().to_degrees();
                    assert!(
                        off_edge > EDGE_ON_DEG,
                        "{}: a ring is seen {off_edge}° off edge-on at {c} s",
                        record.stem
                    );
                }
            }
            frames += 1;
        }
        assert!(frames > 0, "a record ricochets");
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
                let perspective = Knobs {
                    slice_ortho: false,
                    ..Knobs::default()
                };
                let (eye, projection) =
                    framing(&plan, CornerKind::Slice, 0.0, &perspective, &stage);
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
                let (_, projection) =
                    framing(&plan, CornerKind::Slice, 0.0, &Knobs::default(), &stage);
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

    /// The frames that look along the shot.
    const ALONG_SHOT: [CornerKind; 3] =
        [CornerKind::Glass, CornerKind::Victim, CornerKind::Damaged];
    /// The corner's narrowest aspect in these tests: square, stricter than any landscape corner.
    const SQUARE: f32 = 1.0;
    /// Angle tolerance (deg) on the sightline's nudge: DERIVED, f32 rounding of a unit direction
    /// through two rotations is ~1e-5°.
    const NUDGE_TOL_DEG: f32 = 1.0e-2;

    /// Whether `p` lies inside the perspective frustum of the camera at `eye`, at `aspect`.
    fn in_view(eye: &Transform, projection: &Projection, p: Vec3, aspect: f32) -> bool {
        let Projection::Perspective(lens) = projection else {
            panic!("perspective");
        };
        let v = eye.compute_affine().inverse().transform_point3(p);
        let depth = -v.z;
        let tan = (0.5 * lens.fov).tan();
        depth > 0.0 && v.y.abs() <= depth * tan && v.x.abs() <= depth * tan * aspect
    }

    /// The eight corners of `(lo, hi)`.
    fn corners((lo, hi): (Vec3, Vec3)) -> [Vec3; 8] {
        std::array::from_fn(|k| {
            Vec3::new(
                if k & 1 == 0 { lo.x } else { hi.x },
                if k & 2 == 0 { lo.y } else { hi.y },
                if k & 4 == 0 { lo.z } else { hi.z },
            )
        })
    }

    /// The angle (deg) between `a` and `b`.
    fn angle_deg(a: Vec3, b: Vec3) -> f32 {
        a.normalize()
            .dot(b.normalize())
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
    }

    /// Every record, in every frame that looks along the shot, as recorded and with its calibre cut
    /// to 2 mm (fixture data), as the frame opens: the whole tank's bounds and the impact lie in the
    /// field, the camera is upright (its right is level), the sightline to the impact is the shot
    /// turned [`SHOT_NUDGE_DEG`] back toward the shooter, and the frame does not depend on the
    /// calibre.
    #[test]
    fn the_shot_frames_hold_the_whole_tank() {
        let library = Library::load();
        let asset = crate::damage_cam::shapes::TankAsset::load(&library.vehicle).unwrap();
        for record in &library.records {
            let name = record.stem;
            let frames: Vec<Vec<(Transform, Projection)>> =
                [record.report.projectile.caliber_mm, 2.0]
                    .into_iter()
                    .map(|calibre_mm| {
                        let mut report = record.report.clone();
                        report.projectile.caliber_mm = calibre_mm;
                        let plan = Plan::new(&report, &asset.shapes(&report.servo_deg));
                        ALONG_SHOT
                            .iter()
                            .map(|&kind| {
                                framing(&plan, kind, 0.0, &Knobs::default(), &Stage::default())
                            })
                            .collect()
                    })
                    .collect();
            let plan = &record.plan;
            let bounds = plan.tank_bounds.expect("the tank asset has bounds");
            let shot = plan.line.direction_at(plan.contact_arc - 1e-3);
            for (kind, (eye, projection)) in ALONG_SHOT.iter().zip(&frames[0]) {
                for p in corners(bounds) {
                    assert!(
                        in_view(eye, projection, p, SQUARE),
                        "{name} {kind:?}: {p} is out of frame"
                    );
                }
                assert!(
                    in_view(eye, projection, plan.impact_point, SQUARE),
                    "{name} {kind:?}"
                );
                let right = eye.rotation * Vec3::X;
                assert!(
                    right.y.abs() < 1e-5,
                    "{name} {kind:?}: rolled, right {right}"
                );
                let nudge = angle_deg(eye.translation - plan.impact_point, -shot);
                assert!(
                    (nudge - SHOT_NUDGE_DEG).abs() < NUDGE_TOL_DEG,
                    "{name} {kind:?}: the sightline is {nudge}° off the shot"
                );
            }
            for (a, b) in frames[0].iter().zip(&frames[1]) {
                assert!(
                    a.0.translation.distance(b.0.translation) < POSITION_TOL_M,
                    "{name}"
                );
            }
        }
    }

    /// A shot within 1° of straight down (the records' class, at least one) is seen from above the
    /// tank, the impact in the field, with no fallback to a heading it does not have.
    #[test]
    fn a_vertical_shot_is_seen_from_above() {
        let library = Library::load();
        let mut vertical = 0;
        for record in &library.records {
            let plan = &record.plan;
            let shot = plan.line.direction_at(plan.contact_arc - 1e-3);
            if angle_deg(shot, Vec3::NEG_Y) > 1.0 {
                continue;
            }
            let (_, hi) = plan.tank_bounds.expect("the tank asset has bounds");
            for kind in ALONG_SHOT {
                let (eye, projection) =
                    framing(plan, kind, 2.0, &Knobs::default(), &Stage::default());
                assert!(
                    eye.translation.is_finite() && eye.rotation.is_finite(),
                    "{}",
                    record.stem
                );
                assert!(
                    eye.translation.y > hi.y,
                    "{} {kind:?}: eye {}",
                    record.stem,
                    eye.translation
                );
                assert!(
                    in_view(&eye, &projection, plan.impact_point, SQUARE),
                    "{}",
                    record.stem
                );
            }
            vertical += 1;
        }
        assert!(vertical > 0, "a record shoots straight down");
    }

    /// The frames that orbit start on the shot line and turn about the vertical through the tank's
    /// bounds' centre at their rates: the Pen frames at [`timeline::ORBIT_DEG_S`] from the open, the
    /// Damaged frame at [`timeline::DRIFT_DEG_S`] from its hold. Two seconds on, every record's eye
    /// has turned that far about the centre at its distance and height, the tank still in the field;
    /// under reduce motion it has not moved.
    #[test]
    fn the_shot_frames_orbit_the_tank_centre() {
        let library = Library::load();
        let elapsed = 2.0;
        for record in &library.records {
            let plan = &record.plan;
            let bounds = plan.tank_bounds.expect("the tank asset has bounds");
            let center = 0.5 * (bounds.0 + bounds.1);
            for (kind, rate, from) in [
                (CornerKind::Glass, timeline::ORBIT_DEG_S, 0.0),
                (CornerKind::Victim, timeline::ORBIT_DEG_S, 0.0),
                (
                    CornerKind::Damaged,
                    timeline::DRIFT_DEG_S,
                    CornerKind::Damaged.hold_from(),
                ),
            ] {
                let at = |c: f32, reduce: bool| {
                    let knobs = Knobs {
                        reduce_motion: reduce,
                        ..default()
                    };
                    framing(plan, kind, c, &knobs, &Stage::default())
                };
                let (start, _) = at(from, false);
                let (later, projection) = at(from + elapsed, false);
                let (a, b) = (start.translation - center, later.translation - center);
                let turned = Vec2::new(a.x, a.z)
                    .angle_to(Vec2::new(b.x, b.z))
                    .to_degrees()
                    .abs();
                let name = record.stem;
                assert!(
                    (turned - rate * elapsed).abs() < NUDGE_TOL_DEG,
                    "{name} {kind:?}: turned {turned}°"
                );
                assert!((a.length() - b.length()).abs() < POSITION_TOL_M, "{name}");
                assert!((a.y - b.y).abs() < POSITION_TOL_M, "{name}");
                for p in corners(bounds) {
                    assert!(in_view(&later, &projection, p, SQUARE), "{name} {kind:?}");
                }
                let (still, _) = at(from + elapsed, true);
                assert!(
                    still.translation.distance(start.translation) < POSITION_TOL_M,
                    "{name} {kind:?}"
                );
            }
        }
    }

    /// The stage over record `index` in frame `kind`, built by the real [`rebuild`] once: each of
    /// the tank's component shapes stands in for its view mesh, authored a metre back from where the
    /// record has it under a parent a metre forward along `offset`, so only the chain carries it into
    /// the record's frame. Returns the app and each shape's entity, name and bounds' corners in the
    /// record's frame.
    fn staged(
        index: usize,
        kind: CornerKind,
        offset: Vec3,
    ) -> (App, Vec<(Entity, String, [Vec3; 8])>) {
        staged_on(index, kind, offset, super::super::Surface::Corner)
    }

    /// [`staged`] on `surface`: the corner, or the world as the kill cam's lesson draws it.
    fn staged_on(
        index: usize,
        kind: CornerKind,
        offset: Vec3,
        surface: super::super::Surface,
    ) -> (App, Vec<(Entity, String, [Vec3; 8])>) {
        use bevy::camera::primitives::MeshAabb;
        let library = Library::load();
        let record = &library.records[index];
        let asset = crate::damage_cam::shapes::TankAsset::load(&library.vehicle).unwrap();
        let shapes = asset.shapes(&record.report.servo_deg);
        let mut app = App::new();
        let mut meshes = Assets::<Mesh>::default();
        let world = app.world_mut();
        let root = world.spawn(Transform::IDENTITY).id();
        let hull = world.spawn((Transform::IDENTITY, ChildOf(root))).id();
        let parent = world
            .spawn((Transform::from_translation(offset), ChildOf(root)))
            .id();
        let mut placed = Vec::new();
        for (name, shape) in shapes.iter() {
            let mesh = crate::damage_cam::shapes::ComponentShape {
                positions: shape.positions.iter().map(|p| *p - offset).collect(),
                indices: shape.indices.clone(),
            }
            .mesh();
            let Some(aabb) = mesh.compute_aabb() else {
                continue;
            };
            let local = (Vec3::from(aabb.min()), Vec3::from(aabb.max()));
            let entity = world
                .spawn((
                    ViewMesh {
                        root: ViewRoot::Stage,
                        names: vec![name.to_owned()],
                        original: Handle::default(),
                        tier: super::super::view::Tier::Component,
                    },
                    Mesh3d(meshes.add(mesh)),
                    aabb,
                    Transform::IDENTITY,
                    ChildOf(parent),
                ))
                .id();
            placed.push((entity, name.to_owned(), corners(local).map(|p| p + offset)));
        }
        let mut view = StageView::standing_in(Handle::default(), root);
        view.hull = Some(hull);
        app.insert_resource(library.exhibits())
            .insert_resource(Corner {
                showing: Some(super::super::Showing {
                    record: index,
                    turn: 0,
                    kind,
                    c: 0.0,
                    opacity: 1.0,
                    scale: 1.0,
                    remaining: 1.0,
                    surface,
                }),
                fading: None,
            })
            .insert_resource(Stage::default())
            .insert_resource(view)
            .insert_resource(meshes)
            .insert_resource(Assets::<GlowMaterial>::default())
            .insert_resource(Assets::<OutlineMaterial>::default())
            .insert_resource(Assets::<StandardMaterial>::default())
            .insert_resource(Assets::<SliceMaterial>::default())
            .insert_resource(Assets::<LookMaterial>::default())
            .init_resource::<Masks>()
            .add_systems(Update, rebuild);
        app.update();
        (app, placed)
    }

    /// The slice dissolves everything on the eye's side of its plane, a mesh lying wholly on that
    /// side included: the real [`rebuild`] over every slice record gives every one of the tank's
    /// component meshes the slice material, never the glass, whether the plane cuts it or misses
    /// it; at least one record has a mesh wholly on the eye's side.
    #[test]
    fn the_slice_dissolves_what_its_plane_misses() {
        let library = Library::load();
        let (mut wholly, mut cut) = (0, 0);
        for (index, record) in library.records.iter().enumerate() {
            if record.plan.kind != CornerKind::Slice {
                continue;
            }
            let (normal, point) = slice_plane(&record.plan);
            let (app, placed) = staged(index, CornerKind::Slice, normal);
            let world = app.world();
            for (entity, name, corners) in placed {
                let glass = world.get::<MeshMaterial3d<GlowMaterial>>(entity);
                let sliced = world.get::<MeshMaterial3d<SliceMaterial>>(entity).is_some();
                assert!(sliced && glass.is_none(), "{}: {name}", record.stem);
                if corners.iter().all(|p| (*p - point).dot(normal) > 0.0) {
                    wholly += 1;
                } else {
                    cut += 1;
                }
            }
        }
        assert!(wholly > 0 && cut > 0, "{wholly} wholly beyond, {cut} cut");
    }

    /// Whether `color` (linear) is within a small distance of one of the state scale's colours.
    fn on_state_scale(color: Vec3) -> bool {
        [
            timeline::STATE_UNTOUCHED,
            timeline::STATE_RED,
            timeline::STATE_DEAD,
            timeline::STATE_LINE,
        ]
        .into_iter()
        .any(|c| {
            let c = LinearRgba::from(c);
            color.distance(Vec3::new(c.red, c.green, c.blue)) < 0.05
        })
    }

    /// Every record's spall field as the real [`rebuild`] draws it. In every frame but the slice,
    /// the corner's and the kill cam's lesson on the world alike, it is the law's own density,
    /// capped within the round's exit hole, in a cool hue (more blue than red) off the state scale,
    /// its gain one fixed multiple of its cone's strength across every record. In the slice, every
    /// nested shell falls off as 1/r² from the cone's origin in that hue, and the outer shell's
    /// view-angle glow is at most half the nested shells' on-axis sum, so the directions' strengths
    /// shape the field. At least one record throws a cone in each.
    #[test]
    fn the_field_is_cool_and_falls_off_from_its_origin() {
        let library = Library::load();
        let (mut looked, mut sliced) = (0, 0);
        let mut gains: Vec<f32> = Vec::new();
        for (index, record) in library.records.iter().enumerate() {
            let kind = record.plan.kind;
            if record.plan.cones.is_empty() || kind == CornerKind::Ricochet {
                continue;
            }
            for (kind, surface) in [
                (kind, super::super::Surface::Corner),
                (CornerKind::Glass, super::super::Surface::World),
            ] {
                let (mut app, _) = staged_on(index, kind, Vec3::X, surface);
                let world = app.world_mut();
                let fields: Vec<(usize, Handle<LookMaterial>)> = world
                    .query::<(&ConeShell, &MeshMaterial3d<LookMaterial>)>()
                    .iter(world)
                    .map(|(shell, m)| (shell.cone, m.0.clone()))
                    .collect();
                let slice = kind == CornerKind::Slice;
                let want = if slice { 0 } else { record.plan.cones.len() };
                assert_eq!(fields.len(), want, "{} {surface:?}", record.stem);
                for (cone, handle) in &fields {
                    let m = world
                        .resource::<Assets<LookMaterial>>()
                        .get(handle)
                        .unwrap();
                    let color = m.params.color.truncate();
                    assert_eq!(
                        m.params.volume.w,
                        0.5 * record.plan.calibre,
                        "{}: the density is capped within the exit hole",
                        record.stem
                    );
                    gains.push(m.params.color.w / record.plan.cones[*cone].strength);
                    assert!(
                        color.z > color.x && !on_state_scale(color),
                        "{}",
                        record.stem
                    );
                    looked += 1;
                }
                let mut shells = world.query::<(&ConeShell, &MeshMaterial3d<GlowMaterial>)>();
                let glows = world.resource::<Assets<GlowMaterial>>();
                for (cone, _) in record.plan.cones.iter().enumerate() {
                    let materials: Vec<(bool, &GlowMaterial)> = shells
                        .iter(world)
                        .filter(|(shell, _)| shell.cone == cone)
                        .map(|(shell, m)| (shell.rim, glows.get(&m.0).unwrap()))
                        .collect();
                    let want = if slice { CONE_SHELLS + 1 } else { 0 };
                    assert_eq!(materials.len(), want, "{} {surface:?}", record.stem);
                    if !slice {
                        continue;
                    }
                    let mut interior = 0.0;
                    let mut rim = 0.0;
                    for (is_rim, m) in &materials {
                        assert!(m.params.field.y > 0.0, "{}: no falloff", record.stem);
                        let color = m.params.color.truncate();
                        assert!(color.z > color.x, "{}: {color} is warm", record.stem);
                        assert!(
                            !on_state_scale(color),
                            "{}: {color} is on the state scale",
                            record.stem
                        );
                        if *is_rim {
                            rim += m.params.rim.y;
                        } else {
                            interior += m.params.rim.x;
                        }
                    }
                    assert!(
                        rim <= 0.5 * interior,
                        "{}: rim {rim} against {interior}",
                        record.stem
                    );
                    sliced += 1;
                }
            }
        }
        assert!(
            looked > 0 && sliced > 0,
            "{looked} law fields, {sliced} sliced"
        );
        assert!(
            gains
                .iter()
                .all(|gain| (gain / gains[0] - 1.0).abs() < 1e-5),
            "the field's gain is not linear in its strength: {gains:?}"
        );
    }

    /// The see-through hull is the base look wherever it is drawn: in the corner and in the kill
    /// cam's lesson on the world, every one of the tank's meshes the real [`rebuild`] dresses wears
    /// a base look material, none the additive glass, and has its copy in a mask.
    #[test]
    fn the_lesson_draws_the_base_look() {
        let library = Library::load();
        let index = library
            .records
            .iter()
            .position(|record| record.plan.kind == CornerKind::Glass)
            .expect("a record penetrates");
        for surface in [super::super::Surface::Corner, super::super::Surface::World] {
            let (mut app, placed) = staged_on(index, CornerKind::Glass, Vec3::X, surface);
            let world = app.world_mut();
            let copies: Vec<(Entity, VisualScope)> = world
                .query_filtered::<(&ChildOf, &VisualScope), With<StageItem>>()
                .iter(world)
                .map(|(parent, scope)| (parent.parent(), *scope))
                .collect();
            for (entity, name, _) in &placed {
                let world = app.world();
                assert!(
                    world.get::<MeshMaterial3d<LookMaterial>>(*entity).is_some()
                        && world.get::<MeshMaterial3d<GlowMaterial>>(*entity).is_none(),
                    "{surface:?} {name}: not the base look"
                );
                assert!(
                    copies.iter().any(|(parent, scope)| parent == entity
                        && [
                            VisualScope::PEN_CAM_HULL_MASK,
                            VisualScope::PEN_CAM_PART_MASK
                        ]
                        .contains(scope)),
                    "{surface:?} {name}: no mask copy"
                );
            }
        }
    }

    /// Every component the real [`rebuild`] draws, in the shooter's glass and outside view and in
    /// the victim's glass, is on the one state scale under a red outline: a damaged one opaque red
    /// to the share of its hit points lost (the shooter's: this shot's share of what it had; the
    /// victim's: the share of its pool gone), over the untouched grey; a dead one charcoal and
    /// hatched. Across the records at least one of each in each view.
    #[test]
    fn a_component_reads_its_share_of_hit_points_lost() {
        let library = Library::load();
        let (mut damaged, mut dead) = ([0; 2], [0; 2]);
        let rgb = |c: Color| {
            let c = LinearRgba::from(c);
            Vec4::new(c.red, c.green, c.blue, 1.0)
        };
        for (index, record) in library.records.iter().enumerate() {
            for kind in [CornerKind::Glass, CornerKind::Damaged, CornerKind::Victim] {
                let victim = (kind == CornerKind::Victim) as usize;
                let (mut app, _) = staged(index, kind, Vec3::X);
                let world = app.world_mut();
                let outlined: Vec<(usize, Vec4)> = world
                    .query::<(&BoxTint, &MeshMaterial3d<OutlineMaterial>)>()
                    .iter(world)
                    .map(|(tint, m)| {
                        let outlines = world.resource::<Assets<OutlineMaterial>>();
                        (tint.index, outlines.get(&m.0).unwrap().params.color)
                    })
                    .collect();
                let mut tints = world.query::<(&BoxTint, &MeshMaterial3d<LookMaterial>)>();
                let looks = world.resource::<Assets<LookMaterial>>();
                for (tint, material) in tints.iter(world) {
                    let b = &record.plan.boxes[tint.index];
                    let m = looks.get(&material.0).unwrap();
                    let name = format!("{} {kind:?} {}", record.stem, b.name);
                    assert!(
                        outlined.iter().any(|(index, color)| *index == tint.index
                            && color.truncate() == rgb(timeline::STATE_RED).truncate()),
                        "{name}: no red outline"
                    );
                    assert_eq!(m.blend, Blend::Opaque, "{name}: not opaque");
                    assert_eq!(m.params.rest, rgb(timeline::STATE_UNTOUCHED), "{name}");
                    let share = if victim == 1 {
                        b.pool_lost()
                    } else {
                        assert!(b.struck(), "{name}: drawn unstruck");
                        timeline::hp_share(b.lost_hp, b.hp)
                    };
                    if b.lost || share >= 1.0 {
                        assert_eq!(
                            m.params.color.truncate(),
                            rgb(timeline::STATE_DEAD).truncate()
                        );
                        assert_eq!(m.params.state.w, 1.0, "{name}: a dead fill is whole");
                        assert!(m.params.mode.w > 0.5, "{name}: dead and unhatched");
                        dead[victim] += 1;
                        continue;
                    }
                    assert_eq!(
                        m.params.color.truncate(),
                        rgb(timeline::STATE_RED).truncate()
                    );
                    assert_eq!(m.params.state.w, share, "{name}");
                    assert!(m.params.mode.w < 0.5, "{name}: damaged and hatched");
                    damaged[victim] += 1;
                }
            }
        }
        assert!(
            damaged.iter().all(|n| *n > 0) && dead.iter().all(|n| *n > 0),
            "{damaged:?} damaged, {dead:?} dead"
        );
    }

    /// Every record's spall fronts run on the shell's own clock: each starts as the shell's nose
    /// reaches the cone's origin and, while the shell is still moving, stands exactly as far from
    /// the origin as the nose has run along the line since, so it neither lags nor outruns the
    /// shell; a component a cone dosed starts to fill when the front has run the distance the plan
    /// gives it.
    #[test]
    fn the_field_front_keeps_pace_with_the_shell() {
        let library = Library::load();
        let mut fronts = 0;
        for record in &library.records {
            let plan = &record.plan;
            let kind = plan.kind;
            if kind == CornerKind::Ricochet {
                continue;
            }
            for (index, cone) in plan.cones.iter().enumerate() {
                let start = cone_start(plan, kind, index, false);
                assert!(front_radius(plan, kind, index, start).abs() < 1e-5);
                assert!(front_radius(plan, kind, index, start - 0.01) < 0.0);
                let gone = time_at_arc(plan, kind, plan.line.length(), false);
                for k in 1..=16 {
                    let c = start + (gone - start) * k as f32 / 16.0;
                    let run = head_arc(plan, kind, c, false) - cone.arc;
                    let front = front_radius(plan, kind, index, c);
                    assert!(
                        (front - run).abs() < 1e-3,
                        "{} cone {index}: front {front} m, shell {run} m at {c}",
                        record.stem
                    );
                }
                fronts += 1;
            }
            for b in &plan.boxes {
                if let Some(Reach::Cone { cone, distance }) = b.reached_at {
                    let at = box_start(plan, kind, b, false);
                    assert!(
                        (front_radius(plan, kind, cone, at) - distance).abs() < 1e-3,
                        "{}: {}",
                        record.stem,
                        b.name
                    );
                }
            }
        }
        assert!(fronts > 0, "a record throws a cone");
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
