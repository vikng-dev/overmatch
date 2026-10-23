//! The corner's stage: the struck tank's stage view dressed for the frame, and the record drawn
//! over it — the round as the game's shell model at the record's calibre, travelling the recorded
//! path nose first with a thin trail behind it, a ring where it crossed each plate or glanced off a
//! face, every spall cone as its potency band shells in the field's cool hue, cut where its
//! directions stopped ([`look::shell`], and by the plane in the slice), and
//! each struck component's real mesh on the state scale ([`look::component`]): filled red from the
//! bottom up to the share of its HP points lost, one knocked out charcoal and hatched.
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
//! names ([`component_meshes`]), and spawns every record element under them, tagged for their
//! owner. [`Playing`] plays them from a clock: the corner's here ([`animate`]), the x-ray's
//! in the game.

use std::collections::HashMap;

use bevy::camera::ScalingMode;
use bevy::camera::{Hdr, RenderTarget};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

use super::field;
use super::look::{self, FieldPrint, LookMaterial, MaskLayer, MaskSet, Masks};
use super::material::{GlowMaterial, OutlineMaterial, SliceCut, SliceMaterial, SliceParams};
use super::plan::{BoxPlan, Plan, Reach};
use super::potency::{Band, Potency};
use super::timeline::{self, CornerKind};
use super::view::{Proxy, StageView, ViewMesh, ViewRoot};
use super::{Corner, CornerRig, Exhibits, Knobs, WorldViewer};
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
pub(super) const SHELL_SCENE: &str = "shell/shell.glb";
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
/// Those frames hold the tank's bounds this much inside the field: the bounds scaled about their
/// centre by this factor just fit. TUNED.
const BOUNDS_MARGIN: f32 = 1.1;
/// The box fit of those frames checks the orbit at this many spins over a full turn, every 5°.
/// TUNED.
const ORBIT_SAMPLES: u32 = 72;
/// The box fit's bisection steps: DERIVED, they narrow the distance to 2⁻¹⁶ of the sphere's,
/// 0.3 mm of a 20 m sphere distance.
const FIT_STEPS: u32 = 16;
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
#[derive(Component, Clone, Copy)]
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

/// One potency band's shell of cone `cone`, about its origin.
#[derive(Component)]
pub(super) struct ConeShell {
    cone: usize,
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
    /// The mask set the built stage's look is drawn against: its viewer's.
    masks: MaskSet,
    /// The along-shot frames' eye distance ([`shot_distance`]), one solve per record per orbit
    /// value, never per frame: [`animate`] solves it before it frames and re-solves only when the
    /// record on show or the viewer's drag differs from the one it was solved for. `None` (a stage
    /// [`animate`] never ran on, as the tests' `Stage::default()`) makes [`framing`] solve on the
    /// spot; [`animate`]'s path always stores a fit first, so it never does.
    shot_fit: Option<ShotFit>,
    /// The shot fit's solves through [`Stage::fit_shot`].
    #[cfg(test)]
    shot_solves: usize,
}

/// A solved along-shot distance and what it was solved for.
#[derive(Clone, Copy, PartialEq)]
struct ShotFit {
    /// The record's turn and index.
    record: (usize, usize),
    /// The viewer's drag.
    orbit: Vec2,
    distance: f32,
}

impl Stage {
    /// Rebuild at the next [`rebuild`].
    pub(super) fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Store the along-shot distance for `plan` (the record `turn`, `record`) under `orbit`,
    /// solving only when the stored fit is for another record or drag.
    fn fit_shot(&mut self, turn: usize, record: usize, plan: &Plan, orbit: Vec2) {
        let record = (turn, record);
        if self
            .shot_fit
            .is_some_and(|fit| fit.record == record && fit.orbit == orbit)
        {
            return;
        }
        #[cfg(test)]
        {
            self.shot_solves += 1;
        }
        self.shot_fit = Some(ShotFit {
            record,
            orbit,
            distance: shot_distance(plan, orbit),
        });
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

/// Rebuild the stage when the corner's record or frame changes, its look against the mask set of
/// the camera drawing it: the inset's in the corner, the main camera's in the kill cam's lesson.
#[expect(
    clippy::too_many_arguments,
    reason = "rebuilds the stage from the corner, exhibits, view and the viewer's masks into the mesh store and four material stores"
)]
pub fn rebuild(
    mut commands: Commands,
    corner: Res<Corner>,
    exhibits: Res<Exhibits>,
    mut stage: ResMut<Stage>,
    items: Query<Entity, With<StageItem>>,
    view: Res<StageView>,
    tank: Query<(Entity, &ViewMesh, &Mesh3d), Without<Proxy>>,
    main: Query<Entity, With<WorldViewer>>,
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
    let viewer = corner.showing.and_then(|showing| match showing.surface {
        super::Surface::Corner => stage.camera,
        super::Surface::World => main.single().ok(),
    });
    stage.masks = viewer
        .and_then(|viewer| masks.of(viewer))
        .cloned()
        .unwrap_or_default();
    let masks = stage.masks.clone();
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
                    mask_copy(handle.0.clone(), hull_mask.clone(), mask),
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
        (StageItem, VisualScope::PEN_CAM_STAGE),
    );
}

/// A copy of the mesh `mesh` for the mask camera of `layer` alone, drawn with `material` (the
/// masks' [`look::hull_mask`]), at its source's own pose: spawned as the source's child.
pub(super) fn mask_copy(
    mesh: Handle<Mesh>,
    material: Handle<LookMaterial>,
    layer: MaskLayer,
) -> impl Bundle {
    (
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::IDENTITY,
        match layer {
            MaskLayer::Hull => VisualScope::PEN_CAM_HULL_MASK,
            MaskLayer::Part => VisualScope::PEN_CAM_PART_MASK,
        },
    )
}

/// What drawing a record in the base look needs.
pub struct LookCtx<'a> {
    pub looks: &'a mut Assets<LookMaterial>,
    pub masks: &'a MaskSet,
}

/// Draw `plan`'s record over a tank in frame `kind`: the trail, the rings, the spall cones and the
/// components on the state scale. Every element but the tints is spawned under `hull`, an entity
/// whose frame is the record's (the struck hull's); each tint is a copy of one of `components`, the
/// tank's own meshes of the components the record names, spawned under that mesh. `clip` is the
/// slice plane (unit normal toward the eye, a point on it) the cones are cut by. Everything spawned
/// carries `tag` — its owner's marker and the channel it draws on — and is hidden until
/// [`Playing`] places it.
#[expect(
    clippy::too_many_arguments,
    reason = "draws one plan: the spawn sinks, the plan and its frame, the parent and clip, the components, the look and the owner's tag are separate inputs"
)]
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
    tag: impl Bundle + Clone,
) {
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
            tag.clone(),
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
            tag.clone(),
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

    // Spall cones: each one's potency band shells, cut by the slice plane in the slice.
    let potency = Potency::shipped();
    for (index, cone) in plan.cones.iter().enumerate() {
        if matches!(kind, CornerKind::Ricochet) {
            break;
        }
        for band in Band::DRAWN {
            let mut material = look::shell(band, look_ctx.masks, clip);
            material.set_fade(0.0);
            commands.spawn((
                Mesh3d(meshes.add(field::shell_mesh(cone, band, &potency))),
                MeshMaterial3d(look_ctx.looks.add(material)),
                Transform::from_translation(cone.origin),
                Visibility::Hidden,
                ConeShell { cone: index },
                tag.clone(),
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
                tag.clone(),
                ChildOf(component.entity),
            )
        };
        commands.spawn((tint(), MeshMaterial3d(look_ctx.looks.add(fill))));
        commands.spawn((tint(), MeshMaterial3d(outlines.add(outline))));
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
/// cone's front has run the distance the plan gives; the victim's x-ray shows its state from
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
/// that left is never drawn, since it would stand parked in the air at the span's end. A plan
/// with no line ([`Plan::bare`]) draws no shell.
fn shell_drawn(plan: &Plan, kind: CornerKind, c: f32, reduce: bool) -> bool {
    let (from, _) = kind.line_window();
    if c < from || plan.line.points.len() < 2 {
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

/// One record's elements as a frame plays them: `plan` in frame `kind` at corner time `c`, under
/// reduce motion or not, with the shell's head and the trail's end derived once. The corner plays
/// its stage through it ([`animate`]) and the x-ray its record over the live tank; each hands
/// it its own elements, so one clock never reaches the other's.
pub(super) struct Playing<'a> {
    plan: &'a Plan,
    kind: CornerKind,
    c: f32,
    reduce: bool,
    head: f32,
    drawn: bool,
    end: (f32, Vec3),
    line_fade: f32,
}

impl<'a> Playing<'a> {
    pub(super) fn new(plan: &'a Plan, kind: CornerKind, c: f32, reduce: bool) -> Self {
        let head = head_arc(plan, kind, c, reduce);
        let drawn = shell_drawn(plan, kind, c, reduce);
        let end = trail_end(plan, head, drawn);
        let mut playing = Self {
            plan,
            kind,
            c,
            reduce,
            head,
            drawn,
            end,
            line_fade: 1.0,
        };
        if reduce {
            playing.line_fade = playing.fade_from(kind.line_window().0);
        }
        playing
    }

    /// An element's fade-in from corner time `start`: short, longer under reduce motion.
    fn fade_from(&self, start: f32) -> f32 {
        let span = if self.reduce {
            timeline::REDUCED_FADE_S
        } else {
            0.12
        };
        timeline::progress(self.c, start, span)
    }

    /// The shell model's pose, nose at the head at the record's calibre, or `None` while it is not
    /// drawn; the trail ends at its tail.
    pub(super) fn shell(&self) -> Option<Transform> {
        self.drawn.then(|| shell_transform(self.plan, self.head))
    }

    /// A trail leg: along its drawn extent, lit by its brightness, or hidden.
    pub(super) fn line(
        &self,
        seg: &LineSeg,
        material: &MeshMaterial3d<GlowMaterial>,
        transform: &mut Transform,
        visibility: &mut Mut<Visibility>,
        glows: &mut Assets<GlowMaterial>,
    ) {
        let plan = self.plan;
        let Some((a, b)) = trail_span(plan, seg.from, seg.to, self.end) else {
            visibility.set_if_neq(Visibility::Hidden);
            return;
        };
        let span = b - a;
        let length = span.length();
        if length < 1e-5 {
            visibility.set_if_neq(Visibility::Hidden);
            return;
        }
        let trail_width = plan.calibre * TRAIL_CALIBRES;
        *transform = Transform::from_translation(0.5 * (a + b))
            .with_rotation(Quat::from_rotation_arc(Vec3::Y, span / length))
            .with_scale(Vec3::new(trail_width, length, trail_width));
        if let Some(mut m) = glows.get_mut(&material.0) {
            m.set_fade(self.line_fade);
            m.set_intensity(LINE_GLOW * seg.brightness);
        }
        visibility.set_if_neq(Visibility::Inherited);
    }

    /// A ring: faded in as the nose reaches its arc.
    pub(super) fn mark(
        &self,
        mark: &MarkItem,
        material: &MeshMaterial3d<GlowMaterial>,
        visibility: &mut Mut<Visibility>,
        glows: &mut Assets<GlowMaterial>,
    ) {
        let fade = self.fade_from(time_at_arc(self.plan, self.kind, mark.at_arc, self.reduce));
        if let Some(mut m) = glows.get_mut(&material.0) {
            m.set_fade(fade);
        }
        visibility.set_if_neq(if fade > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }

    /// A spall cone's surface: its front run out from the origin, or under reduce motion the whole
    /// field faded in.
    pub(super) fn cone(
        &self,
        shell: &ConeShell,
        look: &MeshMaterial3d<LookMaterial>,
        visibility: &mut Mut<Visibility>,
        looks: &mut Assets<LookMaterial>,
    ) {
        let (plan, kind, c) = (self.plan, self.kind, self.c);
        let (front, fade) = if self.reduce {
            (
                None,
                self.fade_from(cone_start(plan, kind, shell.cone, true)),
            )
        } else {
            let front = front_radius(plan, kind, shell.cone, c);
            (Some(front), if front > 0.0 { 1.0 } else { 0.0 })
        };
        if fade <= 0.0 {
            visibility.set_if_neq(Visibility::Hidden);
            return;
        }
        if let Some(mut m) = looks.get_mut(&look.0) {
            m.set_fade(fade);
            m.set_front(front);
        }
        visibility.set_if_neq(Visibility::Inherited);
    }

    /// Each field's footprint, in world space under the record frame `hull`, on the base look's
    /// surfaces drawn against `masks`; the frames that draw the base look or the tank outside
    /// mark it, the slice does not.
    pub(super) fn footprints(
        &self,
        hull: &GlobalTransform,
        masks: &MaskSet,
        looks: &mut Assets<LookMaterial>,
    ) {
        let (plan, kind, c) = (self.plan, self.kind, self.c);
        if !matches!(
            kind,
            CornerKind::Glass | CornerKind::Victim | CornerKind::Damaged
        ) {
            return;
        }
        let linear = hull.affine().matrix3;
        let prints: Vec<FieldPrint> = plan
            .cones
            .iter()
            .enumerate()
            .map(|(index, cone)| {
                let axis = linear * cone.axis;
                let scale = axis.length();
                let unit = |v: Vec3| {
                    let v = linear * v;
                    v / v.length().max(1e-6)
                };
                let front = if self.reduce {
                    if c >= cone_start(plan, kind, index, true) {
                        cone.max_reach()
                    } else {
                        -1.0
                    }
                } else {
                    front_radius(plan, kind, index, c)
                };
                FieldPrint {
                    origin: hull.transform_point(cone.origin),
                    axis: unit(cone.disc.axis),
                    u: unit(cone.disc.u),
                    v: unit(cone.disc.v),
                    half_angle: cone.disc.half_angle,
                    front: front * scale,
                    scale,
                    nodes: &cone.disc.nodes,
                }
            })
            .collect();
        look::set_footprints(looks, masks, &prints, &Potency::shipped());
    }

    /// A component's state fill and its outline: risen along the up of the record frame `hull`
    /// (the tank's) to the component's heights, filled from the moment the picture reaches it.
    #[expect(
        clippy::too_many_arguments,
        reason = "one tint's two materials and visibility, the two stores and the frame they rise in"
    )]
    pub(super) fn fill(
        &self,
        tint: &BoxTint,
        outline: Option<&MeshMaterial3d<OutlineMaterial>>,
        look: Option<&MeshMaterial3d<LookMaterial>>,
        visibility: &mut Mut<Visibility>,
        hull: Option<&GlobalTransform>,
        outlines: &mut Assets<OutlineMaterial>,
        looks: &mut Assets<LookMaterial>,
    ) {
        let (plan, kind, c) = (self.plan, self.kind, self.c);
        let b = &plan.boxes[tint.index];
        let start = box_start(plan, kind, b, self.reduce);
        let fill = if self.reduce {
            self.fade_from(start)
        } else {
            timeline::ease_enter(timeline::progress(c, start, timeline::BOX_FILL_S))
        };
        if let Some(mut m) = outline.and_then(|m| outlines.get_mut(&m.0)) {
            m.set_fade(fill);
        }
        if let Some(mut m) = look.and_then(|m| looks.get_mut(&m.0)) {
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

    /// Each thrown cone's axis as a line, in the record's frame (the bench's switch).
    fn axis_lines(&self, gizmos: &mut Gizmos<StageGizmos>) {
        let (plan, kind, c) = (self.plan, self.kind, self.c);
        for (index, cone) in plan.cones.iter().enumerate() {
            if c >= cone_start(plan, kind, index, self.reduce) {
                let reach = cone.max_reach();
                gizmos.line(
                    cone.origin,
                    cone.origin + cone.axis * reach,
                    Color::srgb(0.95, 0.95, 0.95),
                );
            }
        }
    }
}

/// Animate the stage from the corner's clock and frame the corner camera.
#[expect(
    clippy::too_many_arguments,
    reason = "one query per disjoint stage element plus four material stores, the queries kept disjoint by their `Without` filters"
)]
pub fn animate(
    corner: Res<Corner>,
    knobs: Res<Knobs>,
    exhibits: Res<Exhibits>,
    mut stage: ResMut<Stage>,
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
        (
            With<StageItem>,
            Without<MarkItem>,
            Without<ConeShell>,
            Without<StageCamera>,
        ),
    >,
    mut marks: Query<
        (&MarkItem, &MeshMaterial3d<GlowMaterial>, &mut Visibility),
        (
            With<StageItem>,
            Without<LineSeg>,
            Without<ConeShell>,
            Without<BoxTint>,
        ),
    >,
    mut shells: Query<
        (&ConeShell, &MeshMaterial3d<LookMaterial>, &mut Visibility),
        (
            With<StageItem>,
            Without<LineSeg>,
            Without<MarkItem>,
            Without<BoxTint>,
        ),
    >,
    mut boxes: Query<
        (
            &BoxTint,
            Option<&MeshMaterial3d<OutlineMaterial>>,
            Option<&MeshMaterial3d<LookMaterial>>,
            &mut Visibility,
        ),
        (
            With<StageItem>,
            Without<LineSeg>,
            Without<MarkItem>,
            Without<ConeShell>,
        ),
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
    let playing = Playing::new(plan, kind, c, knobs.reduce_motion);

    match playing.shell() {
        Some(want) => {
            if *shell_pose != want {
                *shell_pose = want;
            }
            shell_visibility.set_if_neq(Visibility::Inherited);
        }
        None => {
            shell_visibility.set_if_neq(Visibility::Hidden);
        }
    }
    for (seg, material, mut transform, mut visibility) in &mut lines {
        playing.line(seg, material, &mut transform, &mut visibility, &mut glows);
    }
    for (mark, material, mut visibility) in &mut marks {
        playing.mark(mark, material, &mut visibility, &mut glows);
    }
    for (shell, look, mut visibility) in &mut shells {
        playing.cone(shell, look, &mut visibility, &mut looks);
    }
    // The state fills rise along the hull's up, the tank's; the footprints are taken in its frame.
    let hull = stage.hull.and_then(|hull| hulls.get(hull).ok());
    if let Some(hull) = hull {
        playing.footprints(hull, &stage.masks, &mut looks);
    }
    for (tint, outline, look, mut visibility) in &mut boxes {
        playing.fill(
            tint,
            outline,
            look,
            &mut visibility,
            hull,
            &mut outlines,
            &mut looks,
        );
    }
    if knobs.axis_line {
        playing.axis_lines(&mut gizmos);
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

    stage.fit_shot(showing.turn, showing.record, plan, knobs.orbit);
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
    let far = || {
        stage
            .shot_fit
            .map_or_else(|| shot_distance(plan, orbit), |fit| fit.distance)
    };
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
            along_shot(plan, orbit, motion(timeline::ORBIT_DEG_S, 0.0), far())
        }
        CornerKind::Damaged => along_shot(
            plan,
            orbit,
            motion(timeline::DRIFT_DEG_S, kind.hold_from()),
            far(),
        ),
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
                return along_shot(plan, orbit, motion(timeline::DRIFT_DEG_S, 0.0), far());
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
/// The eye stands `far` from the bounds' centre: [`shot_distance`] for this `plan` and `orbit`,
/// which holds through every `spin`.
fn along_shot(plan: &Plan, orbit: Vec2, spin: f32, far: f32) -> (Transform, Projection) {
    let (center, _) = shot_sphere(plan);
    let (toward_eye, up) = shot_sightline(plan, orbit);
    let turn = Quat::from_rotation_y(spin);
    let eye = center + turn * (shot_eye(plan, toward_eye, center, far) - center);
    (
        Transform::from_translation(eye).looking_at(center, turn * up),
        Projection::Perspective(PerspectiveProjection {
            fov: CORNER_FOV_DEG.to_radians(),
            ..default()
        }),
    )
}

/// The along-shot eye's distance from the bounds' centre for `plan` under the viewer's `orbit`.
///
/// It fits the tank's bounding box: the smallest distance on the sightline at which the box,
/// scaled about its centre by [`BOUNDS_MARGIN`], lies inside the vertical field and the corner's
/// horizontal one ([`super::hud::ASPECT`] wider), at every one of [`ORBIT_SAMPLES`] spins of a full
/// turn. Invariant: it does not depend on the spin, so the eye holds it through the whole orbit and
/// the frame never breathes; a full turn covers every spin a frame of any length reaches. Without
/// bounds it fits the sphere around the framed points to the vertical field.
fn shot_distance(plan: &Plan, orbit: Vec2) -> f32 {
    let half_fov = 0.5 * CORNER_FOV_DEG.to_radians();
    let (center, radius) = shot_sphere(plan);
    let (toward_eye, up) = shot_sightline(plan, orbit);
    match plan.tank_bounds {
        Some((lo, hi)) => box_distance(center, 0.5 * (hi - lo), up, half_fov, |far| {
            shot_eye(plan, toward_eye, center, far)
        }),
        None => radius * BOUNDS_MARGIN / half_fov.sin(),
    }
}

/// The along-shot frames' centre and the sphere around it: the tank's bounds', else the framed
/// points'.
fn shot_sphere(plan: &Plan) -> (Vec3, f32) {
    match plan.tank_bounds {
        Some((lo, hi)) => (0.5 * (lo + hi), 0.5 * (hi - lo).length()),
        None => enclose(&plan.framed_points()),
    }
}

/// The along-shot sightline's direction from the impact toward the eye, and the camera's up, for
/// the viewer's `orbit` (see [`along_shot`]).
fn shot_sightline(plan: &Plan, orbit: Vec2) -> (Vec3, Vec3) {
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
    (toward_eye, up)
}

/// The eye on the sightline `toward_eye` from the impact, `far` from `center`: the positive root
/// of `|q + e·t| = far` for `q` the impact off the centre, real while `|q| ≤ far`, which holds for
/// an impact inside the bounds at any distance that fits them.
fn shot_eye(plan: &Plan, toward_eye: Vec3, center: Vec3, far: f32) -> Vec3 {
    let q = plan.impact_point - center;
    let b = toward_eye.dot(q);
    let t = -b + (b * b - q.length_squared() + far * far).max(0.0).sqrt();
    plan.impact_point + toward_eye * t
}

/// The smallest distance from `center` at which the eye `eye_at(distance)`, looking at `center`
/// with `up`, holds the box of half-extents `half` about `center`, scaled by [`BOUNDS_MARGIN`],
/// inside the field of vertical half-angle `half_fov` at the corner's aspect, the box turned about
/// the vertical through `center` to each of [`ORBIT_SAMPLES`] spins of a full turn (turning the box
/// back is turning the eye on).
///
/// A bisection of [`FIT_STEPS`] steps between zero and the box's sphere distance (the scaled
/// sphere's tangent cone at the vertical half-field lies inside the frustum, so that distance
/// fits). It keeps its upper end on a fitting distance, so the result always fits at every sample:
/// conservative, never under; where fitting holds at every distance beyond some edge, it is that
/// edge to within the last step.
fn box_distance(
    center: Vec3,
    half: Vec3,
    up: Vec3,
    half_fov: f32,
    eye_at: impl Fn(f32) -> Vec3,
) -> f32 {
    let tan_v = half_fov.tan();
    let tan_h = tan_v * super::hud::ASPECT;
    let corners: [Vec3; 8] = std::array::from_fn(|k| {
        BOUNDS_MARGIN
            * Vec3::new(
                if k & 1 == 0 { -half.x } else { half.x },
                if k & 2 == 0 { -half.y } else { half.y },
                if k & 4 == 0 { -half.z } else { half.z },
            )
    });
    let fits = |far: f32| {
        let eye = eye_at(far);
        let to_view = Transform::from_translation(eye)
            .looking_at(center, up)
            .rotation
            .inverse();
        (0..ORBIT_SAMPLES).all(|k| {
            let back =
                Quat::from_rotation_y(-std::f32::consts::TAU * k as f32 / ORBIT_SAMPLES as f32);
            corners.iter().all(|&r| {
                let v = to_view * (center + back * r - eye);
                let depth = -v.z;
                depth > 0.0 && v.y.abs() <= depth * tan_v && v.x.abs() <= depth * tan_h
            })
        })
    };
    let (mut lo, mut hi) = (0.0, half.length() * BOUNDS_MARGIN / half_fov.sin());
    for _ in 0..FIT_STEPS {
        let mid = 0.5 * (lo + hi);
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
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
    use super::super::hud::ASPECT;
    use super::super::material::Blend;
    use super::super::plan::LEG_EPS_M;
    use super::*;
    use crate::damage_cam::RoundEnd;
    use crate::damage_cam::shapes::TankAsset;

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

    /// The frames that look along the shot.
    const ALONG_SHOT: [CornerKind; 3] =
        [CornerKind::Glass, CornerKind::Victim, CornerKind::Damaged];
    /// A square field, stricter than any landscape corner, for the points every corner shape holds.
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
                    assert!(in_view(&later, &projection, p, ASPECT), "{name} {kind:?}");
                }
                let (still, _) = at(from + elapsed, true);
                assert!(
                    still.translation.distance(start.translation) < POSITION_TOL_M,
                    "{name} {kind:?}"
                );
            }
        }
    }

    /// Tolerance on the box fit's edge ratio: DERIVED, well above the bisection's last step
    /// ([`FIT_STEPS`], ~2e-5 of the distance) and f32 rounding through the spun frame (~1e-6).
    const FIT_TOL: f32 = 1.0e-3;

    /// Frame `kind` of `plan` at the corner time that has turned it `spin_deg` from the shot line.
    fn spun(plan: &Plan, kind: CornerKind, spin_deg: f32) -> (Transform, Projection) {
        let c = match kind {
            CornerKind::Damaged => kind.hold_from() + spin_deg / timeline::DRIFT_DEG_S,
            _ => spin_deg / timeline::ORBIT_DEG_S,
        };
        framing(plan, kind, c, &Knobs::default(), &Stage::default())
    }

    /// How far toward the field's edge `p` projects in the camera at `eye`, at the corner's
    /// aspect, up and across: 1 on the edge.
    fn edge_ratio(eye: &Transform, projection: &Projection, p: Vec3) -> Vec2 {
        let Projection::Perspective(lens) = projection else {
            panic!("perspective");
        };
        let v = eye.compute_affine().inverse().transform_point3(p);
        let depth = -v.z;
        assert!(depth > 0.0, "{p} behind the eye");
        let tan = (0.5 * lens.fov).tan();
        Vec2::new(
            v.y.abs() / (depth * tan),
            v.x.abs() / (depth * tan * ASPECT),
        )
    }

    /// Every record, in every frame that looks along the shot, through a full turn of its orbit,
    /// with the tank's box as recorded and stretched to three times its longer horizontal side
    /// (fixture data, so the horizontal field binds too): every corner of the box lies in the
    /// corner's field at every degree, the eye holds one distance from the box's centre at every
    /// degree (the frame does not breathe as it orbits), and the fit is tight, the box scaled by
    /// [`BOUNDS_MARGIN`] inside the field at every one of the fit's spins and on its edge at one of
    /// them. The stretched box reaches the horizontal edge for at least one record.
    #[test]
    fn the_shot_frames_fit_the_box_through_the_turn() {
        let library = Library::load();
        let asset = crate::damage_cam::shapes::TankAsset::load(&library.vehicle).unwrap();
        let step = 360 / ORBIT_SAMPLES;
        let mut across = 0;
        for record in &library.records {
            let (lo, hi) = record.plan.tank_bounds.expect("the tank asset has bounds");
            let (center, half) = (0.5 * (lo + hi), 0.5 * (hi - lo));
            let stretch = if half.x >= half.z {
                Vec3::new(3.0, 1.0, 1.0)
            } else {
                Vec3::new(1.0, 1.0, 3.0)
            };
            for (stretched, bounds) in [
                (false, (lo, hi)),
                (true, (center - half * stretch, center + half * stretch)),
            ] {
                let mut plan = Plan::new(&record.report, &asset.shapes(&record.report.servo_deg));
                plan.tank_bounds = Some(bounds);
                let name = format!(
                    "{}{}",
                    record.stem,
                    if stretched { " stretched" } else { "" }
                );
                for kind in ALONG_SHOT {
                    let mut widest = Vec2::ZERO;
                    let open = spun(&plan, kind, 0.0).0.translation.distance(center);
                    for deg in 0..360 {
                        let (eye, projection) = spun(&plan, kind, deg as f32);
                        let distance = eye.translation.distance(center);
                        assert!(
                            (distance - open).abs() < POSITION_TOL_M,
                            "{name} {kind:?} at {deg}°: {distance} m against {open} m"
                        );
                        for p in corners(bounds) {
                            assert!(
                                in_view(&eye, &projection, p, ASPECT),
                                "{name} {kind:?} at {deg}°: {p} is out of frame"
                            );
                        }
                        if deg % step != 0 {
                            continue;
                        }
                        for p in corners(bounds) {
                            let scaled = center + BOUNDS_MARGIN * (p - center);
                            widest = widest.max(edge_ratio(&eye, &projection, scaled));
                        }
                    }
                    assert!(
                        (widest.max_element() - 1.0).abs() < FIT_TOL,
                        "{name} {kind:?}: the scaled box reaches {widest} of the field"
                    );
                    if stretched && (widest.y - 1.0).abs() < FIT_TOL {
                        across += 1;
                    }
                }
            }
        }
        assert!(across > 0, "no stretched box reaches the horizontal edge");
    }

    /// The real [`animate`] solves the along-shot distance once per record and orbit, never per
    /// frame: through a record's frames (its clock running, its frame kind changing) the stage
    /// solves once and the eye stands at the stored distance; a new drag solves once more.
    #[test]
    fn the_shot_fit_is_solved_once_per_record_and_orbit() {
        let index = 0;
        let library = Library::load();
        let asset = TankAsset::load(&library.vehicle).unwrap();
        let (mut app, _) = staged(&library, &asset, index, CornerKind::Glass, Vec3::ZERO);
        let (lo, hi) = app.world().resource::<Exhibits>().0[index]
            .plan
            .tank_bounds
            .expect("the tank asset has bounds");
        let center = 0.5 * (lo + hi);
        // The stage's gizmo group, configured without the render side that draws it.
        let mut gizmos = GizmoConfigStore::default();
        gizmos.insert(GizmoConfig::default(), StageGizmos);
        app.insert_resource(Knobs::default())
            .insert_resource(gizmos)
            .add_systems(Update, animate.after(rebuild));
        app.world_mut().spawn((
            Camera::default(),
            Transform::IDENTITY,
            Projection::Perspective(PerspectiveProjection::default()),
            StageCamera,
        ));
        app.world_mut()
            .spawn((Transform::IDENTITY, Visibility::Hidden, StageShell));
        let frame = |app: &mut App, kind: CornerKind, c: f32| {
            {
                let mut corner = app.world_mut().resource_mut::<Corner>();
                let showing = corner.showing.as_mut().expect("a frame on show");
                (showing.kind, showing.c) = (kind, c);
            }
            app.update();
            let world = app.world_mut();
            let eye = world
                .query_filtered::<&Transform, With<StageCamera>>()
                .single(world)
                .expect("the inset camera")
                .translation;
            let stage = world.resource::<Stage>();
            let fit = stage.shot_fit.expect("animate stores a fit");
            assert!(
                (eye.distance(center) - fit.distance).abs() < POSITION_TOL_M,
                "{kind:?} at {c}: the eye stands {} m off, the fit {} m",
                eye.distance(center),
                fit.distance
            );
            stage.shot_solves
        };
        for step in 0..30 {
            let kind = if step < 20 {
                CornerKind::Glass
            } else {
                CornerKind::Damaged
            };
            assert_eq!(frame(&mut app, kind, 0.2 * step as f32), 1, "frame {step}");
        }
        app.world_mut().resource_mut::<Knobs>().orbit = Vec2::new(0.3, 0.1);
        for step in 30..40 {
            let c = 0.2 * step as f32;
            assert_eq!(frame(&mut app, CornerKind::Damaged, c), 2, "frame {step}");
        }
    }

    /// The stage over record `index` in frame `kind`, built by the real [`rebuild`] once: each of
    /// the tank's component shapes stands in for its view mesh, authored a metre back from where the
    /// record has it under a parent a metre forward along `offset`, so only the chain carries it into
    /// the record's frame. `library` holds the records and `asset` their tank, both loaded once by
    /// the caller. Returns the app and each shape's entity, name and bounds' corners in the
    /// record's frame.
    fn staged(
        library: &Library,
        asset: &TankAsset,
        index: usize,
        kind: CornerKind,
        offset: Vec3,
    ) -> (App, Vec<(Entity, String, [Vec3; 8])>) {
        staged_on(
            library,
            asset,
            index,
            kind,
            offset,
            super::super::Surface::Corner,
        )
    }

    /// [`staged`] on `surface`: the corner, or the world as the kill cam's lesson draws it.
    fn staged_on(
        library: &Library,
        asset: &TankAsset,
        index: usize,
        kind: CornerKind,
        offset: Vec3,
        surface: super::super::Surface,
    ) -> (App, Vec<(Entity, String, [Vec3; 8])>) {
        use bevy::camera::primitives::MeshAabb;
        let record = &library.records[index];
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
        let asset = TankAsset::load(&library.vehicle).unwrap();
        let (mut wholly, mut cut) = (0, 0);
        for (index, record) in library.records.iter().enumerate() {
            if record.plan.kind != CornerKind::Slice {
                continue;
            }
            let (normal, point) = slice_plane(&record.plan);
            let (app, placed) = staged(&library, &asset, index, CornerKind::Slice, normal);
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

    /// Every record's spall field as the real [`rebuild`] draws it, in every frame, the corner's and
    /// the kill cam's lesson on the world alike: each cone draws one shell per potency band, each in
    /// its band's colour on one cool ladder — more blue than red, off the state scale, each band
    /// brighter than the one below it — added at one gain across every record, so the colour says
    /// the band and nothing else, never the cone's strength. Outside the slice a shell is uncut and
    /// drawn over everything, through the glass; in the slice it is cut by the slice plane and drawn
    /// under the tank the slice keeps. At least one record throws a cone in each.
    #[test]
    fn the_field_is_its_bands_on_one_cool_ladder() {
        let library = Library::load();
        let asset = TankAsset::load(&library.vehicle).unwrap();
        let (mut looked, mut sliced) = (0, 0);
        let mut gains: Vec<f32> = Vec::new();
        let tint = |band: Band| {
            let c = LinearRgba::from(timeline::field_tint(band));
            Vec3::new(c.red, c.green, c.blue)
        };
        let luminance = |c: Vec3| c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        for pair in Band::DRAWN.windows(2) {
            assert!(
                luminance(tint(pair[0])) < luminance(tint(pair[1])),
                "{pair:?}"
            );
        }
        for (index, record) in library.records.iter().enumerate() {
            let kind = record.plan.kind;
            if record.plan.cones.is_empty() || kind == CornerKind::Ricochet {
                continue;
            }
            for (kind, surface) in [
                (kind, super::super::Surface::Corner),
                (CornerKind::Glass, super::super::Surface::World),
            ] {
                let (mut app, _) = staged_on(&library, &asset, index, kind, Vec3::X, surface);
                let world = app.world_mut();
                let fields: Vec<(usize, Handle<LookMaterial>)> = world
                    .query::<(&ConeShell, &MeshMaterial3d<LookMaterial>)>()
                    .iter(world)
                    .map(|(shell, m)| (shell.cone, m.0.clone()))
                    .collect();
                assert_eq!(
                    fields.len(),
                    3 * record.plan.cones.len(),
                    "{} {surface:?}",
                    record.stem
                );
                let slice = kind == CornerKind::Slice;
                let cut = world.resource::<Stage>().plane;
                assert_eq!(cut.is_some(), slice, "{} {surface:?}", record.stem);
                let looks = world.resource::<Assets<LookMaterial>>();
                for (cone, _) in record.plan.cones.iter().enumerate() {
                    let mut colors: Vec<Vec3> = Vec::new();
                    for (_, handle) in fields.iter().filter(|(of, _)| *of == cone) {
                        let m = looks.get(handle).unwrap();
                        let color = m.params.color.truncate();
                        assert!(
                            color.z > color.x && !on_state_scale(color),
                            "{}: {color}",
                            record.stem
                        );
                        match cut {
                            Some((normal, point)) => {
                                assert_eq!(m.params.mode.y, 1.0, "{}", record.stem);
                                assert_eq!(m.params.plane, normal.extend(normal.dot(point)));
                                assert!(!m.unoccluded && m.bias, "{}", record.stem);
                            }
                            None => {
                                assert_eq!(m.params.mode.y, 0.0, "{}", record.stem);
                                assert!(m.unoccluded, "{}", record.stem);
                            }
                        }
                        gains.push(m.params.color.w);
                        colors.push(color);
                    }
                    for band in Band::DRAWN {
                        assert_eq!(
                            colors.iter().filter(|c| **c == tint(band)).count(),
                            1,
                            "{} {surface:?}: {band:?}",
                            record.stem
                        );
                    }
                    if slice {
                        sliced += 1;
                    } else {
                        looked += 1;
                    }
                }
            }
        }
        assert!(
            looked > 0 && sliced > 0,
            "{looked} fields through the glass, {sliced} sliced"
        );
        assert!(
            gains.iter().all(|gain| *gain == gains[0]),
            "a shell's gain varies across the records: {gains:?}"
        );
    }

    /// Every component the real [`rebuild`] draws, in the shooter's glass and outside view and in
    /// the victim's glass, is on the one state scale under a red outline: a damaged one opaque red
    /// to the share of its hit points lost (the shooter's: this shot's share of what it had; the
    /// victim's: the share of its pool gone), over the untouched grey; a dead one charcoal and
    /// hatched. Across the records at least one of each in each view.
    #[test]
    fn a_component_reads_its_share_of_hit_points_lost() {
        let library = Library::load();
        let asset = TankAsset::load(&library.vehicle).unwrap();
        let (mut damaged, mut dead) = ([0; 2], [0; 2]);
        let rgb = |c: Color| {
            let c = LinearRgba::from(c);
            Vec4::new(c.red, c.green, c.blue, 1.0)
        };
        for (index, record) in library.records.iter().enumerate() {
            for kind in [CornerKind::Glass, CornerKind::Damaged, CornerKind::Victim] {
                let victim = (kind == CornerKind::Victim) as usize;
                let (mut app, _) = staged(&library, &asset, index, kind, Vec3::X);
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
            (e(4), vec![none.clone(), inner, none.clone(), outer]),
            (e(5), vec![none]),
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
