//! The corner's stage: the struck tank's stage view dressed for the frame, and the record drawn
//! over it — the round as the game's shell model at the record's calibre, travelling the recorded
//! path nose first with a thin trail behind it, a ring where it crossed each plate or glanced off a
//! face, the record's fields (each spall cone, and a burst's) as one translucent volume in the
//! potency bands of their summed intensity, bounded where their directions stopped
//! ([`look::volume`], and by the plane in the slice), and each component's real mesh on the state
//! scale ([`look::component`]): filled red from the bottom up to the share of its whole HP pool
//! gone, rising at its reach time from its state before the shot to its state after it, one
//! knocked out charcoal and hatched.
//! The shell rests where the record ends; a round the record has leave the tank runs on past the
//! path's last node along its last leg for the rest of the play ([`shell_share`]). A round that
//! bursts breaks up at the burst's record time: only a ring's nose piece flies on, the model
//! shortened to its share, and a few pieces of the round ride the burst field's front, each along
//! one of its directions to that direction's end ([`Piece`]). The nose and every field's front
//! move on the record's own clock at one constant slowdown ([`timeline::REPLAY_SLOWDOWN`]): the
//! nose along the record's timed path, a spall cone's front at the round's exit speed, a burst's
//! at its sideways speed ([`super::plan::Cone::speed`]), each from its record time. [`pace`] is a
//! record's corner times in a frame. The rule, the burst's beat and the frames' minimum lengths:
//! `demo/ux-spec.md` §3, "Time".
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
use bevy::ecs::system::SystemParam;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot, WorldInstanceReady};

use super::field;
use super::look::{self, FieldPrint, Layer, LookMaterial, MaskSet, Masks};
use super::material::{GlowMaterial, OutlineMaterial, SliceCut, SliceMaterial, SliceParams};
use super::plan::{BoxPlan, BurstPlan, Plan, Reach};
use super::potency::Potency;
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
/// A piece of a burst: a shard's length along its direction and its width, in calibres. TUNED: a
/// diagram choice, small beside the shell.
const PIECE_CALIBRES: (f32, f32) = (0.5, 0.2);
/// The most pieces a burst throws: an even subset of its field's directions. TUNED.
const PIECES_MAX: usize = 32;
/// A piece shrinks away over this many corner seconds from the moment it stops. TUNED.
const PIECE_FADE_S: f32 = 0.5;
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

/// A piece of the round's burst, riding direction `ray` of the burst's field, cone `cone`. `cut`
/// is the slice plane (unit normal toward the eye, a point on it): the piece is never drawn on the
/// eye's side, where the slice cuts the field away.
#[derive(Component)]
pub(super) struct Piece {
    cone: usize,
    ray: usize,
    cut: Option<(Vec3, Vec3)>,
}

/// Everything [`rebuild`] spawns for one record.
#[derive(Component, Clone, Copy)]
pub(super) struct StageItem;

/// One leg of the trail between arc lengths `from` and `to`; `to` is unbounded on the last leg of a
/// round that left, which runs on behind the shell past the path's last node.
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

/// The record's fields' one volume, on the box about their reach ([`look::field_bounds`]).
#[derive(Component)]
pub(super) struct FieldVolume;

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
    /// Whether the built stage predates a change it depends on: the masks its materials are made
    /// against, or the round's meshes landing ([`round_landed`]).
    stale: bool,
    /// The slice plane: unit normal toward the eye, and a point on it.
    plane: Option<(Vec3, Vec3)>,
    /// The slice's material for each of the tank's own materials on each layer: a material the
    /// body and a component share takes one for each.
    slices: HashMap<(AssetId<StandardMaterial>, Layer), Handle<SliceMaterial>>,
    camera: Option<Entity>,
    /// The hull the built stage is drawn under: its frame is the record's, its up the tank's.
    hull: Option<Entity>,
    /// The mask set the built stage's look is drawn against: its viewer's.
    masks: MaskSet,
    /// The round's own material, which a burst's pieces wear ([`shell_material`]).
    round_material: Handle<StandardMaterial>,
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
    stage.round_material = shell_material(asset_server);
    commands
        .spawn((
            WorldAssetRoot(shell),
            Transform::IDENTITY,
            Visibility::Hidden,
            VisualScope::PEN_CAM_STAGE,
            StageShell,
            CornerRig,
        ))
        .observe(round_landed);
}

/// The material the shell model wears: bevy_pbr's glTF handler adds each glTF material's
/// `StandardMaterial` under `<label>/std` and gives a mesh of that material that handle
/// (`bevy_pbr-0.19.1/src/gltf.rs`, `on_material`); the model has one material, unscaled (MEASURED).
pub(super) fn shell_material(asset_server: &AssetServer) -> Handle<StandardMaterial> {
    let label = GltfAssetLabel::Material {
        index: 0,
        is_scale_inverted: false,
    };
    asset_server.load(format!("{SHELL_SCENE}#{label}/std"))
}

/// The round's meshes have landed: the stage is built again, so they get their mask copies.
fn round_landed(_ready: On<WorldInstanceReady>, mut stage: ResMut<Stage>) {
    stage.invalidate();
}

/// The round's model on the stage, for [`rebuild`] to copy into the part mask.
#[derive(SystemParam)]
pub struct StageRound<'w, 's> {
    shells: Query<'w, 's, Entity, With<StageShell>>,
    children: Query<'w, 's, &'static Children>,
    /// Not the copies a rebuild despawns, which hang under the round's meshes.
    meshes: Query<'w, 's, &'static Mesh3d, Without<StageItem>>,
}

impl StageRound<'_, '_> {
    /// Each of the round's meshes, with its mesh.
    fn meshes(&self) -> impl Iterator<Item = (Entity, Handle<Mesh>)> + '_ {
        self.shells
            .iter()
            .flat_map(|shell| self.children.iter_descendants(shell))
            .filter_map(|entity| Some((entity, self.meshes.get(entity).ok()?.0.clone())))
    }
}

/// The round on the stage as [`animate`] places it: the shell model and a burst's pieces.
#[derive(SystemParam)]
pub struct StageRoundPose<'w, 's> {
    shell: Query<
        'w,
        's,
        (&'static mut Transform, &'static mut Visibility),
        (
            With<StageShell>,
            Without<LineSeg>,
            Without<MarkItem>,
            Without<FieldVolume>,
            Without<BoxTint>,
            Without<StageCamera>,
        ),
    >,
    pieces: Query<
        'w,
        's,
        (
            &'static Piece,
            &'static mut Transform,
            &'static mut Visibility,
        ),
        (
            With<StageItem>,
            Without<StageShell>,
            Without<LineSeg>,
            Without<MarkItem>,
            Without<FieldVolume>,
            Without<BoxTint>,
            Without<StageCamera>,
        ),
    >,
}

/// Rebuild the stage when the corner's record or frame changes, its look against the mask set of
/// the camera drawing it: the inset's in the corner, the main camera's in the kill cam's lesson.
#[expect(
    clippy::too_many_arguments,
    reason = "rebuilds the stage from the corner, exhibits, view, round and the viewer's masks into the mesh and image stores and four material stores"
)]
pub fn rebuild(
    mut commands: Commands,
    corner: Res<Corner>,
    exhibits: Res<Exhibits>,
    mut stage: ResMut<Stage>,
    items: Query<Entity, With<StageItem>>,
    view: Res<StageView>,
    tank: Query<(Entity, &ViewMesh, &Mesh3d), Without<Proxy>>,
    round: StageRound,
    main: Query<Entity, With<WorldViewer>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut glows: ResMut<Assets<GlowMaterial>>,
    mut outlines: ResMut<Assets<OutlineMaterial>>,
    standard: Res<Assets<StandardMaterial>>,
    mut slices: ResMut<Assets<SliceMaterial>>,
    (mut looks, mut images): (ResMut<Assets<LookMaterial>>, ResMut<Assets<Image>>),
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
    let mut tier_materials: HashMap<super::view::Tier, (Handle<LookMaterial>, Layer)> =
        HashMap::new();
    let hull_mask = looks.add(look::hull_mask(&masks));
    // The slice's part mask holds each component's kept side alone, as the slice draws it.
    let cut_mask = stage.plane.map(|plane| {
        let mut material = look::hull_mask(&masks);
        material.set_cut(Some(plane));
        looks.add(material)
    });

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
                    (looks.add(draw.material), draw.layer)
                });
                let (material, layer) = (entry.0.clone(), entry.1);
                e.insert((MeshMaterial3d(material), Visibility::Inherited));
                commands.spawn((
                    mask_copy(handle.0.clone(), hull_mask.clone(), layer),
                    StageItem,
                    ChildOf(entity),
                ));
            }
            CornerKind::Slice => {
                let layer = Layer::of(mesh.tier);
                let (normal, point) = stage.plane.unwrap_or((Vec3::Z, Vec3::ZERO));
                let params = SliceParams {
                    plane: normal.extend(normal.dot(point)),
                    cut_color: LinearRgba::from(CUT).to_vec4(),
                    params: Vec4::new(0.0, 8.0, 0.3, (layer == Layer::Body) as u8 as f32),
                };
                let material = stage
                    .slices
                    .entry((mesh.original.id(), layer))
                    .or_insert_with(|| {
                        let mut base = standard.get(&mesh.original).cloned().unwrap_or_default();
                        base.cull_mode = None;
                        base.double_sided = true;
                        slices.add(SliceMaterial {
                            base,
                            extension: SliceCut {
                                params,
                                part_mask: None,
                            },
                        })
                    })
                    .clone();
                if let Some(mut slice) = slices.get_mut(&material) {
                    slice.extension.params = params;
                    slice.extension.part_mask = Some(masks.part.clone());
                }
                e.insert((MeshMaterial3d(material), Visibility::Inherited));
                if let (Layer::Scene, Some(cut_mask)) = (layer, &cut_mask) {
                    commands.spawn((
                        mask_copy(handle.0.clone(), cut_mask.clone(), Layer::Scene),
                        StageItem,
                        ChildOf(entity),
                    ));
                }
            }
            CornerKind::Ricochet | CornerKind::Damaged => {
                e.insert((MeshMaterial3d(mesh.original.clone()), Visibility::Inherited));
            }
        }
    }
    if look::body_behind(kind) {
        round_copies(&mut commands, round.meshes(), &hull_mask, StageItem);
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
            images: &mut images,
            masks: &masks,
        },
        &stage.round_material,
        (StageItem, VisualScope::PEN_CAM_STAGE),
    );
}

/// A copy of the mesh `mesh` for the mask camera of `layer` alone (the body's the hull mask, the
/// scene's the part mask), drawn with `material` (the masks' [`look::hull_mask`]), at its source's
/// own pose: spawned as the source's child.
pub(super) fn mask_copy(
    mesh: Handle<Mesh>,
    material: Handle<LookMaterial>,
    layer: Layer,
) -> impl Bundle {
    (
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::IDENTITY,
        match layer {
            Layer::Body => VisualScope::PEN_CAM_HULL_MASK,
            Layer::Scene => VisualScope::PEN_CAM_PART_MASK,
        },
    )
}

/// The round's mask copies, one under each of its meshes (`meshes`, each with its mesh), on
/// [`Layer::ROUND`]'s side, each carrying `tag`. `material` is the viewer's masks' material
/// uncut: the round is drawn whole in every frame, the slice's included. A copy takes its mesh's
/// pose and visibility, so a hidden round bounds no field.
pub(super) fn round_copies(
    commands: &mut Commands,
    meshes: impl IntoIterator<Item = (Entity, Handle<Mesh>)>,
    material: &Handle<LookMaterial>,
    tag: impl Bundle + Clone,
) {
    for (entity, mesh) in meshes {
        commands.spawn((
            mask_copy(mesh, material.clone(), Layer::ROUND),
            tag.clone(),
            ChildOf(entity),
        ));
    }
}

/// What drawing a record in the base look needs.
pub struct LookCtx<'a> {
    pub looks: &'a mut Assets<LookMaterial>,
    pub images: &'a mut Assets<Image>,
    pub masks: &'a MaskSet,
}

/// Draw `plan`'s record over a tank in frame `kind`: the trail, the rings, the fields, a burst's
/// pieces and the components on the state scale. Every element but the tints is spawned under
/// `hull`, an entity whose frame is the record's (the struck hull's); each tint is a copy of one of
/// `components`, the tank's own meshes of the components the record names, spawned under that
/// mesh. `clip` is the slice plane (unit normal toward the eye, a point on it) the fields and the
/// pieces are cut by. The pieces wear `round_material`, the round's own ([`shell_material`]).
/// Everything spawned carries `tag` — its owner's marker and the channel it draws on — and is
/// hidden until [`Playing`] places it.
#[expect(
    clippy::too_many_arguments,
    reason = "draws one plan: the spawn sinks, the plan and its frame, the parent and clip, the components, the look, the round's material and the owner's tag are separate inputs"
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
    round_material: &Handle<StandardMaterial>,
    tag: impl Bundle + Clone,
) {
    // The trail, one cylinder per leg of the path from the drawn span's start; a round that left
    // trails on past the span behind the shell, and its last leg past the path's last node.
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
        let to = if plan.left() && i + 1 == line.points.len() {
            f32::INFINITY
        } else {
            to
        };
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

    // The fields: their maps in one atlas, and one volume on the box about their reach, cut by
    // the slice plane in the slice.
    let maps = plan.maps();
    if kind != CornerKind::Ricochet
        && let Some((lo, hi)) = look::field_bounds(&plan.cones, maps)
    {
        let atlas = look_ctx.images.add(field::atlas(maps));
        let mut material = look::volume(&Potency::shipped(), look_ctx.masks, kind, clip, atlas);
        material.set_fade(0.0);
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::from_corners(lo, hi))),
            MeshMaterial3d(look_ctx.looks.add(material)),
            Transform::from_translation(0.5 * (lo + hi)),
            Visibility::Hidden,
            FieldVolume,
            tag.clone(),
            ChildOf(hull),
        ));
    }

    // A burst's pieces: one mesh and the round's material shared by all, no light, each on the
    // round's side of the draw order where the frame draws the body behind, as the round is.
    if let Some(index) = plan.burst.as_ref().and_then(|burst| burst.cone)
        && kind != CornerKind::Ricochet
    {
        let shard = meshes.add(Tetrahedron::default());
        let mask =
            look::body_behind(kind).then(|| look_ctx.looks.add(look::hull_mask(look_ctx.masks)));
        for ray in piece_rays(plan.cones[index].rays.len()) {
            let mut piece = commands.spawn((
                Mesh3d(shard.clone()),
                MeshMaterial3d(round_material.clone()),
                Transform::IDENTITY,
                Visibility::Hidden,
                Piece {
                    cone: index,
                    ray,
                    cut: clip,
                },
                tag.clone(),
                ChildOf(hull),
            ));
            if let Some(mask) = &mask {
                piece.with_child(mask_copy(shard.clone(), mask.clone(), Layer::ROUND));
            }
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

/// The corner time at which the record reaches record time `t` (s) in frame `kind`: the nose stands
/// at the drawn span's start at the frame's travel start, and the record runs on from there at
/// [`timeline::REPLAY_SLOWDOWN`].
fn corner_at(plan: &Plan, kind: CornerKind, t: f32) -> f32 {
    kind.travel_start() + (t - plan.line.time_at(plan.start_arc)) * timeline::REPLAY_SLOWDOWN
}

/// The arc length of the shell's nose at corner time `c`: the record's own arc at the record time
/// `c` stands for ([`corner_at`]), so it crosses each leg at that leg's record speed scaled and
/// stops where the record's path ends; a round that left runs on past the path's last node along
/// its last leg at that leg's speed ([`super::plan::Polyline::arc_at_time`]). Reduce motion puts it
/// at the span's end when the travel starts.
fn head_arc(plan: &Plan, kind: CornerKind, c: f32, reduce: bool) -> f32 {
    let from = kind.travel_start();
    if reduce {
        return if c >= from {
            plan.end_arc
        } else {
            plan.start_arc
        };
    }
    let t = plan.line.time_at(plan.start_arc) + (c - from).max(0.0) / timeline::REPLAY_SLOWDOWN;
    let head = plan.line.arc_at_time(t).max(plan.start_arc);
    if plan.left() {
        head
    } else {
        head.min(plan.line.length())
    }
}

/// The corner time at which the nose reaches arc `s`.
fn time_at_arc(plan: &Plan, kind: CornerKind, s: f32, reduce: bool) -> f32 {
    if reduce {
        return kind.travel_start();
    }
    corner_at(plan, kind, plan.line.time_at(s.max(plan.start_arc)))
}

/// The corner time at which cone `index` is thrown: its record time.
fn cone_start(plan: &Plan, kind: CornerKind, index: usize, reduce: bool) -> f32 {
    if reduce {
        return kind.travel_start();
    }
    corner_at(plan, kind, plan.cones[index].t)
}

/// How far (m) cone `index`'s front has run from its origin at corner time `c`, negative before the
/// cone is thrown: its own speed ([`super::plan::Cone::speed`]) over the record time since it was
/// thrown, so on screen it runs at that speed over [`timeline::REPLAY_SLOWDOWN`], whatever the
/// line's length.
fn front_radius(plan: &Plan, kind: CornerKind, index: usize, c: f32) -> f32 {
    let dt = (c - cone_start(plan, kind, index, false)) / timeline::REPLAY_SLOWDOWN;
    plan.cones[index].front(dt)
}

/// The corner time at which component `b`'s fill starts, in every frame: the nose's entry, or the
/// moment the dosing cone's front has run the distance the plan gives at that front's own speed;
/// never for a component the picture does not reach.
fn box_start(plan: &Plan, kind: CornerKind, b: &BoxPlan, reduce: bool) -> f32 {
    match b.reached_at {
        Some(Reach::Line(arc)) => time_at_arc(plan, kind, arc, reduce),
        Some(Reach::Cone { cone, distance }) => {
            if reduce {
                cone_start(plan, kind, cone, true)
            } else {
                cone_start(plan, kind, cone, false)
                    + plan.cones[cone].run_time(distance) * timeline::REPLAY_SLOWDOWN
            }
        }
        None => f32::INFINITY,
    }
}

/// `plan`'s corner times in frame `kind`: its travel from the frame's travel start to the nose at
/// the drawn span's end, its settle — that, each cone's front at its longest reach, each of a
/// burst's pieces gone and each dosed component's fill complete — and the hold and length that
/// follow ([`timeline::Pace`]). A plan with no line travels nowhere.
pub(super) fn pace(plan: &Plan, kind: CornerKind) -> timeline::Pace {
    let travel_end = if plan.line.points.len() < 2 {
        kind.travel_start()
    } else {
        time_at_arc(plan, kind, plan.end_arc, false)
    };
    let cones = plan.cones.iter().enumerate().map(|(index, cone)| {
        cone_start(plan, kind, index, false)
            + cone.run_time(cone.max_reach()) * timeline::REPLAY_SLOWDOWN
    });
    let pieces = plan
        .burst
        .iter()
        .filter_map(|burst| burst.cone)
        .flat_map(|index| {
            piece_rays(plan.cones[index].rays.len())
                .map(move |ray| piece_stop(plan, kind, index, ray) + PIECE_FADE_S)
        });
    let boxes = plan
        .boxes
        .iter()
        .map(|b| box_start(plan, kind, b, false) + timeline::BOX_FILL_S)
        .filter(|t| t.is_finite());
    let settle = cones.chain(pieces).chain(boxes).fold(travel_end, f32::max);
    timeline::Pace::new(kind, travel_end, settle)
}

/// The corner time at which `burst` breaks the shell up: its record time, or under reduce motion
/// the travel start, where every element shows whole.
fn burst_start(plan: &Plan, kind: CornerKind, burst: &BurstPlan, reduce: bool) -> f32 {
    if reduce {
        kind.travel_start()
    } else {
        corner_at(plan, kind, burst.t)
    }
}

/// The share of the shell model's length drawn at corner time `c`, `None` while none is: the
/// whole model from the travel start; from a burst on ([`burst_start`]) only the nose piece where
/// one flies on, at its share of the round ([`BurstPlan::nose`]), and nothing where none does (the
/// ball, a round that burst at rest). A round the record has leave the tank runs on past its
/// path's last node ([`head_arc`]) and no frame follows it, so nothing hides it in flight: its
/// bound is the play's end, the corner's or the lesson's (the x-ray's clock stops at the hold, and
/// the shell with it). Under reduce motion a round that left is never drawn, since it would stand
/// parked in the air at the span's end. A plan with no line ([`Plan::bare`]) draws no shell.
fn shell_share(plan: &Plan, kind: CornerKind, c: f32, reduce: bool) -> Option<f32> {
    if c < kind.travel_start() || plan.line.points.len() < 2 || (reduce && plan.left()) {
        return None;
    }
    match &plan.burst {
        Some(burst) if c >= burst_start(plan, kind, burst, reduce) => burst.nose,
        _ => Some(1.0),
    }
}

/// The shell model's transform with its nose at arc `head`, `share` of its length drawn: the tip
/// on the line, the body along the chord to the line's point that much of a model length back,
/// scaled from the model's calibre to the record's and along its body by `share`. A nose piece is
/// drawn as the model's front shortened to its share of the round's mass: a DERIVED visual proxy
/// that treats the round's mass per length as even. On a straight leg the chord is the travel;
/// within a model length past a bend the body pivots across it, so past a bounce it stays on the
/// face's outer side as the path does, where the travel's own direction would stand it inside the
/// face.
fn shell_transform(plan: &Plan, head: f32, share: f32) -> Transform {
    let nose = plan.line.at(head);
    let back = plan.line.at(head - share * shell_length(plan));
    let body = (nose - back)
        .try_normalize()
        .unwrap_or_else(|| plan.line.direction_at(head));
    let scale = plan.calibre / SHELL_MODEL_CALIBRE_M;
    Transform::from_translation(nose)
        .looking_to(body, Vec3::Y)
        .with_scale(Vec3::new(scale, scale, scale * share))
}

/// The shell model's length (m) at the record's calibre.
fn shell_length(plan: &Plan) -> f32 {
    SHELL_MODEL_LENGTH_M * plan.calibre / SHELL_MODEL_CALIBRE_M
}

/// Where the trail ends: its arc along the line and its point. Behind a drawn shell (`share` of
/// the model, [`shell_share`]) it ends at the model's tail, which within a model length past a
/// bend stands just off the line, the body's chord across the bend being shorter than the model;
/// otherwise at the nose.
fn trail_end(plan: &Plan, head: f32, share: Option<f32>) -> (f32, Vec3) {
    let Some(share) = share else {
        return (head, plan.line.at(head));
    };
    let tail = shell_transform(plan, head, share).transform_point(Vec3::Z * SHELL_MODEL_LENGTH_M);
    (head - share * shell_length(plan), tail)
}

/// The directions of a field of `count` that a burst's pieces ride, by index: an even subset, at
/// most [`PIECES_MAX`].
fn piece_rays(count: usize) -> impl Iterator<Item = usize> {
    let pieces = count.min(PIECES_MAX);
    (0..pieces).map(move |k| k * count / pieces)
}

/// The corner time at which the piece on direction `ray` of cone `index` stops: its front reaches
/// the direction's recorded end.
fn piece_stop(plan: &Plan, kind: CornerKind, index: usize, ray: usize) -> f32 {
    let cone = &plan.cones[index];
    cone_start(plan, kind, index, false)
        + cone.run_time(cone.rays[ray].reach) * timeline::REPLAY_SLOWDOWN
}

/// The drawn extent of the trail leg between arcs `from` and `to`, cut at the trail's end: the
/// leg holding that end runs to its point, so an unbounded `to` always ends there.
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
    share: Option<f32>,
    end: (f32, Vec3),
    line_fade: f32,
}

impl<'a> Playing<'a> {
    pub(super) fn new(plan: &'a Plan, kind: CornerKind, c: f32, reduce: bool) -> Self {
        let head = head_arc(plan, kind, c, reduce);
        let share = shell_share(plan, kind, c, reduce);
        let end = trail_end(plan, head, share);
        let mut playing = Self {
            plan,
            kind,
            c,
            reduce,
            head,
            share,
            end,
            line_fade: 1.0,
        };
        if reduce {
            playing.line_fade = playing.fade_from(kind.travel_start());
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

    /// The shell model's pose, nose at the head at the record's calibre and the share of it drawn,
    /// or `None` while it is not drawn; the trail ends at its tail.
    pub(super) fn shell(&self) -> Option<Transform> {
        self.share
            .map(|share| shell_transform(self.plan, self.head, share))
    }

    /// A piece of a burst: placed, or hidden while [`Self::piece_pose`] draws none.
    pub(super) fn piece(
        &self,
        piece: &Piece,
        transform: &mut Mut<Transform>,
        visibility: &mut Mut<Visibility>,
    ) {
        match self.piece_pose(piece) {
            Some(pose) => {
                transform.set_if_neq(pose);
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }

    /// A piece's pose in the record's frame, or `None` while it is not drawn: from its field's
    /// throw (the burst), on its direction from the field's origin at the field's front
    /// ([`front_radius`]), so the pieces are where the field's volume ends; stopped at the
    /// direction's recorded end and shrunk away over [`PIECE_FADE_S`] from the moment it stops
    /// ([`piece_stop`]). A shard of [`PIECE_CALIBRES`] at the record's calibre, its length along
    /// the direction. Never under reduce motion, nor on the eye's side of the slice plane.
    fn piece_pose(&self, piece: &Piece) -> Option<Transform> {
        let (plan, kind, c) = (self.plan, self.kind, self.c);
        if self.reduce || c < cone_start(plan, kind, piece.cone, false) {
            return None;
        }
        let cone = &plan.cones[piece.cone];
        let ray = &cone.rays[piece.ray];
        let at =
            cone.origin + ray.direction * front_radius(plan, kind, piece.cone, c).min(ray.reach);
        if piece
            .cut
            .is_some_and(|(normal, point)| (at - point).dot(normal) > 0.0)
        {
            return None;
        }
        let gone = timeline::progress(
            c,
            piece_stop(plan, kind, piece.cone, piece.ray),
            PIECE_FADE_S,
        );
        if gone >= 1.0 {
            return None;
        }
        let size = 1.0 - timeline::ease_exit(gone);
        let (length, width) = PIECE_CALIBRES;
        Some(
            Transform::from_translation(at)
                .with_rotation(Quat::from_rotation_arc(Vec3::Y, ray.direction))
                .with_scale(Vec3::new(width, length, width) * plan.calibre * size),
        )
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

    /// The fields' volume (the spall cones' and a burst's), in world space under the record frame
    /// `hull` (hidden without one): each field's front run out from its origin, or under reduce
    /// motion each whole field faded in.
    pub(super) fn fields(
        &self,
        look: &MeshMaterial3d<LookMaterial>,
        visibility: &mut Mut<Visibility>,
        hull: Option<&GlobalTransform>,
        looks: &mut Assets<LookMaterial>,
    ) {
        let (plan, kind, c) = (self.plan, self.kind, self.c);
        let fade = (0..plan.cones.len())
            .map(|index| {
                if self.reduce {
                    self.fade_from(cone_start(plan, kind, index, true))
                } else if front_radius(plan, kind, index, c) > 0.0 {
                    1.0
                } else {
                    0.0
                }
            })
            .fold(0.0, f32::max);
        let Some(hull) = hull.filter(|_| fade > 0.0) else {
            visibility.set_if_neq(Visibility::Hidden);
            return;
        };
        if let Some(mut m) = looks.get_mut(&look.0) {
            m.set_fields(&self.prints(hull), &Potency::shipped());
            m.set_fade(fade);
        }
        visibility.set_if_neq(Visibility::Inherited);
    }

    /// Every field in world space under the record frame `hull`: its origin, its map's frame and
    /// zoom, and its front run out from the origin, or under reduce motion its map's longest reach
    /// once it is thrown and negative before.
    fn prints(&self, hull: &GlobalTransform) -> Vec<FieldPrint> {
        let (plan, kind, c) = (self.plan, self.kind, self.c);
        let linear = hull.affine().matrix3;
        let unit = |v: Vec3| {
            let v = linear * v;
            v / v.length().max(1e-6)
        };
        plan.cones
            .iter()
            .zip(plan.maps().iter())
            .enumerate()
            .map(|(index, (cone, map))| {
                let scale = (linear * map.axis).length();
                let front = if self.reduce {
                    if c >= cone_start(plan, kind, index, true) {
                        map.longest
                    } else {
                        -1.0
                    }
                } else {
                    front_radius(plan, kind, index, c)
                };
                FieldPrint {
                    origin: hull.transform_point(cone.origin),
                    axis: unit(map.axis),
                    u: unit(map.u),
                    v: unit(map.v),
                    half_angle: map.half_angle,
                    inner: map.inner,
                    zoom: map.zoom,
                    front: front * scale,
                    scale,
                    longest: map.longest,
                }
            })
            .collect()
    }

    /// A component's state fill and its outline: risen along the up of the record frame `hull`
    /// (the tank's) to the component's heights, at its condition of the moment
    /// ([`look::condition`]): its state before the shot until the picture reaches it, then rising
    /// over [`timeline::BOX_FILL_S`] to its state after it. Shown while it has a condition; the
    /// outline whole wherever the component had one before the shot, else fading in with the rise.
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
        let run = if self.reduce {
            self.fade_from(start)
        } else {
            timeline::progress(c, start, timeline::BOX_FILL_S)
        };
        // Whole exactly when its run is: the ease's solve lands within a tolerance of its end.
        let rise = if run >= 1.0 || self.reduce {
            run
        } else {
            timeline::ease_enter(run)
        };
        let (before, condition) = (look::condition(b, 0.0), look::condition(b, rise));
        if let Some(mut m) = outline.and_then(|m| outlines.get_mut(&m.0)) {
            m.set_fade(if before.is_some() { 1.0 } else { rise });
        }
        if let Some(mut m) = look.and_then(|m| looks.get_mut(&m.0)) {
            m.set_fade(condition.is_some() as u8 as f32);
            if let Some(condition) = condition {
                m.set_condition(condition);
            }
            if let Some(hull) = hull {
                let up = *hull.up();
                let height = |y: f32| hull.transform_point(Vec3::Y * y).dot(up);
                m.set_fill_span(up, b.heights.map(|(lo, hi)| (height(lo), height(hi))));
            }
        }
        visibility.set_if_neq(if condition.is_some() {
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
            Without<FieldVolume>,
            Without<StageCamera>,
        ),
    >,
    mut marks: Query<
        (&MarkItem, &MeshMaterial3d<GlowMaterial>, &mut Visibility),
        (
            With<StageItem>,
            Without<LineSeg>,
            Without<FieldVolume>,
            Without<BoxTint>,
        ),
    >,
    mut volumes: Query<
        (&MeshMaterial3d<LookMaterial>, &mut Visibility),
        (
            With<StageItem>,
            With<FieldVolume>,
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
            Without<FieldVolume>,
        ),
    >,
    mut camera: Query<
        (&mut Camera, &mut Transform, &mut Projection),
        (With<StageCamera>, Without<LineSeg>, Without<FieldVolume>),
    >,
    mut round: StageRoundPose,
    mut gizmos: Gizmos<StageGizmos>,
) {
    let Ok((mut camera, mut eye, mut projection)) = camera.single_mut() else {
        return;
    };
    let Ok((mut shell_pose, mut shell_visibility)) = round.shell.single_mut() else {
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
    for (piece, mut transform, mut visibility) in &mut round.pieces {
        playing.piece(piece, &mut transform, &mut visibility);
    }
    for (seg, material, mut transform, mut visibility) in &mut lines {
        playing.line(seg, material, &mut transform, &mut visibility, &mut glows);
    }
    for (mark, material, mut visibility) in &mut marks {
        playing.mark(mark, material, &mut visibility, &mut glows);
    }
    // The state fills rise along the hull's up, the tank's; the fields are taken in its frame.
    let hull = stage.hull.and_then(|hull| hulls.get(hull).ok());
    for (look, mut visibility) in &mut volumes {
        playing.fields(look, &mut visibility, hull, &mut looks);
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
    let hold_from = pace(plan, kind).hold_from;
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
        CornerKind::Damaged => {
            along_shot(plan, orbit, motion(timeline::DRIFT_DEG_S, hold_from), far())
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
            let drift = motion(timeline::DRIFT_DEG_S, hold_from);
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
    use super::super::plan::{LEAD_M, LEG_EPS_M};
    use super::super::timeline::Condition;
    use super::*;
    use crate::damage_cam::shapes::{TankAsset, TankShapes};
    use crate::damage_cam::{PenetrationReport, RoundEnd};

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

    /// For every record: the shell sets off from the drawn span's start at its frame's travel
    /// start, reaches the span's end at its pace's travel end, and is drawn nose first on the line
    /// at the record's calibre, along its body at the share of it drawn; a round the record has
    /// rest keeps its nose on the record's last point to the end of the frame, drawn whole or, past
    /// a burst, as what of it the record has fly on (`a_burst_draws_only_what_flies_on`), and one
    /// the record has leave is never drawn under reduce motion (its run past the path:
    /// `a_round_that_left_runs_on_past_its_last_node`).
    #[test]
    fn the_shell_ends_where_the_record_ends() {
        let library = Library::load();
        for record in &library.records {
            let plan = &record.plan;
            let kind = plan.kind;
            let pace = pace(plan, kind);
            let (from, to) = (pace.travel_start, pace.travel_end);
            let name = record.stem;
            assert_eq!(from, kind.travel_start(), "{name}");
            assert!(
                (head_arc(plan, kind, from, false) - plan.start_arc).abs() < 1e-5,
                "{name}"
            );
            assert!(
                (head_arc(plan, kind, to, false) - plan.end_arc).abs() < 1e-3,
                "{name}"
            );
            let samples = 16;
            let step = (to - from) / samples as f32;
            for k in 0..=samples {
                let c = from + step * k as f32;
                let head = head_arc(plan, kind, c, false);
                let Some(share) = shell_share(plan, kind, c, false) else {
                    continue;
                };
                let pose = shell_transform(plan, head, share);
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
                assert_eq!(pose.scale.y, pose.scale.x, "{name}");
                assert!((pose.scale.z - pose.scale.x * share).abs() < 1e-6, "{name}");
            }
            let last = *plan.line.points.last().unwrap();
            match record.report.end {
                RoundEnd::Left { .. } => {
                    assert!(shell_share(plan, kind, to, true).is_none(), "{name}");
                }
                RoundEnd::Rest { .. } | RoundEnd::Burst { .. } => {
                    // The whole shell, or what of it a burst left flying.
                    let left = match &record.report.burst {
                        Some(_) => flies_on(&record.report),
                        None => Some(1.0),
                    };
                    for reduce in [false, true] {
                        let end = pace.duration;
                        let share = shell_share(plan, kind, end, reduce);
                        assert!(same_share(share, left), "{name}: {share:?}, not {left:?}");
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

    /// A round the record has leave the tank runs on past its path's last node, which is where the
    /// record stops logging its flight, not where the round stopped: from the corner time its nose
    /// reaches that node the shell stays drawn, its nose on the last leg produced at that leg's
    /// record speed over [`timeline::REPLAY_SLOWDOWN`] and its body along it. The corner, the
    /// lesson and the x-ray place the shell by this. Each leaving record plays as recorded and cut
    /// 5 cm past the point it left (fixture data), its last node then well inside the frame, so
    /// neither a nose clamped to the last node nor a shell hidden there passes. The trail
    /// [`spawn_glass`] lays runs on to the shell's tail, so a trail that stops at the last node
    /// fails too.
    #[test]
    fn a_round_that_left_runs_on_past_its_last_node() {
        use crate::damage_cam::PathPoint;
        let library = Library::load();
        let asset = TankAsset::load(&library.vehicle).unwrap();
        let mut leaving = 0;
        for (index, record) in library.records.iter().enumerate() {
            let RoundEnd::Left { at } = record.report.end else {
                continue;
            };
            let at = Vec3::from(at);
            let mut cut = record.report.clone();
            let k = cut
                .path
                .iter()
                .position(|p| Vec3::from(p.point).distance(at) < 1.0e-5)
                .expect("where the round left is a point of its path");
            let next = cut.path[k + 1];
            let leg = Vec3::from(next.point) - at;
            cut.path.truncate(k + 1);
            cut.path.push(PathPoint {
                t: cut.path[k].t + (next.t - cut.path[k].t) * 0.05 / leg.length(),
                point: (at + leg.normalize() * 0.05).to_array(),
            });
            let cut_plan = Plan::new(&cut, &asset.shapes(&cut.servo_deg));
            let name = record.stem;
            for (plan, played) in [(&*record.plan, "as recorded"), (&cut_plan, "cut")] {
                let line = &plan.line;
                let n = line.points.len() - 1;
                let (direction, speed) = (line.leg_direction(n), line.leg_speed(n).unwrap());
                for kind in [CornerKind::Glass, plan.kind] {
                    let reached = time_at_arc(plan, kind, line.length(), false);
                    for k in 1..=8 {
                        let c = reached + 0.25 * k as f32;
                        let Some(share) = shell_share(plan, kind, c, false) else {
                            panic!("{name} {played}: hidden at {c} s, past its last node");
                        };
                        let head = head_arc(plan, kind, c, false);
                        let run = (c - reached) / timeline::REPLAY_SLOWDOWN * speed;
                        assert!(
                            (head - line.length() - run).abs() < 1e-3,
                            "{name} {played}: {} m past the last node, not {run}",
                            head - line.length()
                        );
                        let pose = shell_transform(plan, head, share);
                        let nose = line.points[n] + direction * (head - line.length());
                        assert!(
                            pose.translation.distance(nose) < POSITION_TOL_M,
                            "{name} {played}: the nose is off the last leg"
                        );
                        assert!(
                            (pose.rotation * Vec3::NEG_Z).dot(direction) > 1.0 - 1e-5,
                            "{name} {played}: the body is off the last leg"
                        );
                    }
                    assert!(
                        shell_share(plan, kind, reached + 1.0, true).is_none(),
                        "{name}"
                    );
                }
            }
            let plan = &record.plan;
            let (mut app, _) = staged(&library, &asset, index, plan.kind, Vec3::ZERO);
            let c = time_at_arc(plan, plan.kind, plan.line.length(), false) + 0.5;
            let head = head_arc(plan, plan.kind, c, false);
            let (tail_arc, tail) = trail_end(plan, head, shell_share(plan, plan.kind, c, false));
            assert!(tail_arc > plan.line.length(), "{name}");
            let world = app.world_mut();
            let mut segs = world.query::<&LineSeg>();
            let reaches_tail = segs.iter(world).any(|seg| {
                trail_span(plan, seg.from, seg.to, (tail_arc, tail))
                    .is_some_and(|(_, b)| b.distance(tail) < POSITION_TOL_M)
            });
            assert!(reaches_tail, "{name}: the trail stops short of the tail");
            leaving += 1;
        }
        assert!(leaving > 0, "a record leaves the tank");
    }

    /// Every record's nose crosses each leg of its drawn span at that leg's own record speed over
    /// [`timeline::REPLAY_SLOWDOWN`]: read off the record's path (length over the derived time
    /// span), so the shell slows exactly where the record says it slowed and nowhere else. The
    /// corner, the kill cam's lesson and the x-ray all place the shell by this clock. The library
    /// holds a record whose legs differ in speed by more than a quarter, so neither a span-over-
    /// window pace nor one speed for every leg passes.
    #[test]
    fn the_nose_runs_each_leg_at_its_record_speed() {
        let library = Library::load();
        let mut legs = 0;
        let mut spread: f32 = 1.0;
        for record in &library.records {
            let plan = &record.plan;
            let kind = plan.kind;
            let name = record.stem;
            let mut speeds: Vec<f32> = Vec::new();
            for pair in record.report.path.windows(2) {
                let (a, b) = (Vec3::from(pair[0].point), Vec3::from(pair[1].point));
                let dt = pair[1].t - pair[0].t;
                if dt <= 0.0 || a.distance(b) < 1e-3 {
                    continue;
                }
                let speed = a.distance(b) / dt;
                let lo = plan.line.arc_of(a).max(plan.start_arc);
                let hi = plan.line.arc_of(b).min(plan.end_arc);
                if hi - lo < 1e-2 {
                    continue;
                }
                let (c0, c1) = (
                    time_at_arc(plan, kind, lo, false),
                    time_at_arc(plan, kind, hi, false),
                );
                let (ca, cb) = (c0 + 0.25 * (c1 - c0), c0 + 0.75 * (c1 - c0));
                let shown =
                    (head_arc(plan, kind, cb, false) - head_arc(plan, kind, ca, false)) / (cb - ca);
                let want = speed / timeline::REPLAY_SLOWDOWN;
                assert!(
                    (shown - want).abs() <= 1e-3 * want,
                    "{name}: {shown} m per corner-s on a {speed} m/s leg"
                );
                speeds.push(speed);
                legs += 1;
            }
            if let (Some(fast), Some(slow)) = (
                speeds.iter().copied().reduce(f32::max),
                speeds.iter().copied().reduce(f32::min),
            ) {
                spread = spread.max(fast / slow);
            }
        }
        assert!(legs > 0, "a record crosses a leg");
        assert!(
            spread > 1.25,
            "the library's legs differ in speed: {spread}"
        );
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
                let pose = shell_transform(plan, head, 1.0);
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

    /// The share of the round the record has fly on from its burst, read off the record: a ring's
    /// nose mass over the round's, from a burst in flight; none under the ball or from a round
    /// that burst at rest.
    fn flies_on(report: &PenetrationReport) -> Option<f32> {
        let burst = report.burst.as_ref()?;
        burst
            .nose_kg
            .filter(|_| burst.speed_mps > 0.0)
            .map(|kg| kg / report.projectile.mass_kg)
    }

    /// Whether two drawn shares are the same, to f32 rounding.
    fn same_share(a: Option<f32>, b: Option<f32>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => (a - b).abs() < 1e-6,
            (a, b) => a.is_none() && b.is_none(),
        }
    }

    /// From the corner time the record bursts on, the shell is drawn only as what flies on: under
    /// the ball, and for a round that burst at rest, nothing; under a ring in flight its nose
    /// piece, the model's front at the nose's share of the round's mass read off the record, its
    /// nose on the line where the record's clock has it, parked at the path's end. Before the
    /// burst the whole model is drawn. The corner, the lesson and the x-ray place the shell by
    /// this. The library holds a ball, a round that burst at rest and a ring whose path runs on
    /// past its burst point by more than a model length (fixture data), so neither a shell drawn
    /// whole to the end, a nose piece drawn full length, nor one parked at the burst point passes.
    #[test]
    fn a_burst_draws_only_what_flies_on() {
        let library = Library::load();
        let (mut balls, mut at_rest, mut rings) = (0, 0, 0);
        for record in &library.records {
            let Some(burst) = &record.report.burst else {
                continue;
            };
            let plan = &record.plan;
            let name = record.stem;
            let flies_on = flies_on(&record.report);
            let last = *plan.line.points.last().unwrap();
            for kind in [CornerKind::Glass, plan.kind] {
                let pace = pace(plan, kind);
                let bursts = corner_of(plan, kind, burst.t);
                let before = bursts - 1e-3;
                if before >= kind.travel_start() {
                    let drawn = shell_share(plan, kind, before, false);
                    assert!(
                        same_share(drawn, Some(1.0)),
                        "{name}: {drawn:?} before the burst"
                    );
                }
                let samples = 16;
                for k in 0..=samples {
                    let c = bursts + (pace.duration - bursts) * k as f32 / samples as f32;
                    let shell = Playing::new(plan, kind, c, false).shell();
                    let Some(share) = flies_on else {
                        assert!(
                            shell.is_none(),
                            "{name}: a shell drawn at {c} s, past its burst"
                        );
                        continue;
                    };
                    let pose = shell.unwrap_or_else(|| panic!("{name}: no nose piece at {c} s"));
                    let head = head_arc(plan, kind, c, false);
                    assert!(
                        pose.translation.distance(plan.line.at(head)) < POSITION_TOL_M,
                        "{name}: the nose piece is off the line at {c} s"
                    );
                    let drawn = pose.scale.z / pose.scale.x;
                    assert!(
                        (drawn - share).abs() < 1e-5,
                        "{name}: the nose piece is {drawn} of the model, not {share}"
                    );
                }
                if flies_on.is_some() {
                    let parked = Playing::new(plan, kind, pace.duration, false).shell();
                    let nose = parked.map_or(Vec3::INFINITY, |pose| pose.translation);
                    assert!(
                        nose.distance(last) < 1e-4,
                        "{name}: the nose piece parks {} m off the path's end",
                        nose.distance(last)
                    );
                }
            }
            match flies_on {
                None if burst.speed_mps > 0.0 => balls += 1,
                None => at_rest += 1,
                Some(share) => {
                    assert!(share < 1.0, "{name}: the nose piece is the whole round");
                    if last.distance(Vec3::from(burst.point)) > shell_length(plan) {
                        rings += 1;
                    }
                }
            }
        }
        assert!(
            balls > 0 && at_rest > 0 && rings > 0,
            "{balls} balls, {at_rest} at rest, {rings} rings flying on"
        );
    }

    /// The corner time at which record time `t` plays in frame `kind`, from the test's own reading
    /// of the clock: the drawn span's start at the frame's travel start, then
    /// [`timeline::REPLAY_SLOWDOWN`].
    fn corner_of(plan: &Plan, kind: CornerKind, t: f32) -> f32 {
        kind.travel_start() + (t - plan.line.time_at(plan.start_arc)) * timeline::REPLAY_SLOWDOWN
    }

    /// A burst's pieces ride its field: each on a recorded direction
    /// ([`crate::damage_cam::Fragment::end`]) from the field's origin, at the front the record
    /// gives — the burst's sideways speed over the record time since the field was thrown
    /// ([`crate::damage_cam::Burst::sideways_mps`]) — and never past the direction's end, where it
    /// stops and then shrinks away within [`PIECE_FADE_S`]; none before the burst, and every one
    /// gone by the settle, so the hold never freezes one mid-flight. A record's sideways speed
    /// differs from the round's carried speed (fixture data), so a piece run at the shell's pace
    /// fails, as does one run on past its end.
    #[test]
    fn a_bursts_pieces_ride_its_field_front() {
        let library = Library::load();
        let (mut fields, mut differ) = (0, 0);
        for record in &library.records {
            let Some((burst, field)) = record
                .report
                .burst
                .as_ref()
                .and_then(|burst| Some((burst, burst.field.as_ref()?)))
            else {
                continue;
            };
            let plan = &record.plan;
            let name = record.stem;
            let index = plan
                .burst
                .as_ref()
                .and_then(|b| b.cone)
                .expect("the field is drawn");
            let cone = &plan.cones[index];
            let origin = Vec3::from(field.origin);
            for kind in [CornerKind::Glass, plan.kind] {
                let thrown = corner_of(plan, kind, field.t);
                let settle = pace(plan, kind).settle;
                for ray in piece_rays(cone.rays.len()) {
                    let piece = Piece {
                        cone: index,
                        ray,
                        cut: None,
                    };
                    let direction = cone.rays[ray].direction;
                    let reach = cone.rays[ray].reach;
                    let end = origin + direction * reach;
                    assert!(
                        field
                            .fragments
                            .iter()
                            .any(|f| Vec3::from(f.end).distance(end) < POSITION_TOL_M),
                        "{name}: piece {ray} rides no recorded direction"
                    );
                    let pose = |c: f32| Playing::new(plan, kind, c, false).piece_pose(&piece);
                    assert!(
                        pose(thrown - 1e-3).is_none(),
                        "{name}: piece {ray} before the burst"
                    );
                    let stops = thrown + reach / burst.sideways_mps * timeline::REPLAY_SLOWDOWN;
                    assert!(
                        stops + PIECE_FADE_S <= settle + 1e-4,
                        "{name}: piece {ray} outlasts the settle"
                    );
                    let samples = 32;
                    let span = stops + PIECE_FADE_S + 0.1 - thrown;
                    let mut drawn = 0;
                    for k in 0..=samples {
                        let c = thrown + span * k as f32 / samples as f32;
                        let Some(pose) = pose(c) else {
                            assert!(
                                c >= stops + PIECE_FADE_S - 1e-4,
                                "{name}: piece {ray} hidden at {c} s, before it is gone"
                            );
                            continue;
                        };
                        assert!(
                            c < stops + PIECE_FADE_S + 1e-4,
                            "{name}: piece {ray} lingers"
                        );
                        let t = field.t + (c - thrown) / timeline::REPLAY_SLOWDOWN;
                        let front = burst.sideways_mps * (t - field.t);
                        let want = origin + direction * front.min(reach);
                        assert!(
                            pose.translation.distance(want) < POSITION_TOL_M,
                            "{name}: piece {ray} at {c} s stands {} m off the front",
                            pose.translation.distance(want)
                        );
                        assert!(
                            pose.translation.distance(origin) <= reach + POSITION_TOL_M,
                            "{name}: piece {ray} past its end"
                        );
                        drawn += 1;
                    }
                    assert!(
                        drawn > samples / 2,
                        "{name}: piece {ray} drawn {drawn} times"
                    );
                }
            }
            if (burst.sideways_mps - burst.speed_mps).abs() > 1.0 {
                differ += 1;
            }
            fields += 1;
        }
        assert!(
            fields > 0 && differ > 0,
            "{fields} burst fields, {differ} faster than the round"
        );
    }

    /// In the slice no piece is drawn on the eye's side of the plane, where the slice cuts the
    /// field away. A burst record the slice frames throws pieces to both sides of its plane
    /// (fixture data), so neither an ignored cut nor one that hides every piece passes.
    #[test]
    fn the_slice_cuts_a_bursts_pieces() {
        let library = Library::load();
        let (mut kept, mut cut) = (0, 0);
        for record in &library.records {
            let plan = &record.plan;
            let Some(index) = plan.burst.as_ref().and_then(|b| b.cone) else {
                continue;
            };
            let (normal, point) = slice_plane(plan);
            let thrown = cone_start(plan, CornerKind::Slice, index, false);
            for ray in piece_rays(plan.cones[index].rays.len()) {
                let whole = Piece {
                    cone: index,
                    ray,
                    cut: None,
                };
                let sliced = Piece {
                    cut: Some((normal, point)),
                    ..whole
                };
                for k in 0..=16 {
                    let c = thrown + 0.05 * k as f32;
                    let playing = Playing::new(plan, CornerKind::Slice, c, false);
                    match (playing.piece_pose(&whole), playing.piece_pose(&sliced)) {
                        (Some(_), Some(pose)) => {
                            assert!(
                                (pose.translation - point).dot(normal) <= 0.0,
                                "{}: piece {ray} on the eye's side",
                                record.stem
                            );
                            kept += 1;
                        }
                        (Some(_), None) => cut += 1,
                        (None, None) => {}
                        (None, Some(_)) => panic!("{}: the cut draws piece {ray}", record.stem),
                    }
                }
            }
        }
        assert!(kept > 0 && cut > 0, "{kept} kept, {cut} cut");
    }

    /// Under reduce motion a burst throws no piece at any time, and the shell shows what the burst
    /// leaves from the travel start on: the nose piece parked at the path's end, or nothing (as
    /// for any round that left).
    #[test]
    fn reduce_motion_draws_no_piece() {
        let library = Library::load();
        let mut bursts = 0;
        for record in &library.records {
            let plan = &record.plan;
            let Some(index) = plan.burst.as_ref().and_then(|b| b.cone) else {
                continue;
            };
            let name = record.stem;
            for kind in [CornerKind::Glass, plan.kind] {
                let pace = pace(plan, kind);
                let left = shell_share(plan, kind, kind.travel_start(), true);
                let want = flies_on(&record.report).filter(|_| !plan.left());
                assert!(same_share(left, want), "{name}: {left:?}, not {want:?}");
                for k in 0..=32 {
                    let c = pace.duration * k as f32 / 32.0;
                    let playing = Playing::new(plan, kind, c, true);
                    for ray in piece_rays(plan.cones[index].rays.len()) {
                        let piece = Piece {
                            cone: index,
                            ray,
                            cut: None,
                        };
                        assert!(
                            playing.piece_pose(&piece).is_none(),
                            "{name}: piece {ray} drawn at {c} s under reduce motion"
                        );
                    }
                }
            }
            bursts += 1;
        }
        assert!(bursts > 0, "a record bursts with a field");
    }

    /// The real [`rebuild`] throws a burst's pieces as an even subset of its field's directions, at
    /// most [`PIECES_MAX`], each hidden until played, all on one mesh and the stage's round
    /// material ([`shell_material`]): never a mesh or a material per piece. A field whose every
    /// direction ended where it was thrown (a round that burst at rest inside a plate) throws none.
    #[test]
    fn a_bursts_pieces_share_one_mesh_and_the_rounds_material() {
        let library = Library::load();
        let asset = TankAsset::load(&library.vehicle).unwrap();
        let mut built = 0;
        for (index, record) in library.records.iter().enumerate() {
            let Some(cone) = record.plan.burst.as_ref().and_then(|b| b.cone) else {
                continue;
            };
            let count = record.plan.cones[cone].rays.len();
            let name = record.stem;
            let (mut app, _) = staged(&library, &asset, index, record.plan.kind, Vec3::ZERO);
            let world = app.world_mut();
            let round = world.resource::<Stage>().round_material.id();
            let mut pieces = world.query::<(
                &Piece,
                &Mesh3d,
                &MeshMaterial3d<StandardMaterial>,
                &Visibility,
            )>();
            let mut rays = Vec::new();
            let mut meshes = std::collections::HashSet::new();
            for (piece, mesh, material, visibility) in pieces.iter(world) {
                assert_eq!(material.0.id(), round, "{name}");
                assert_eq!(*visibility, Visibility::Hidden, "{name}");
                meshes.insert(mesh.0.id());
                rays.push(piece.ray);
            }
            rays.sort_unstable();
            assert_eq!(rays.len(), count.min(PIECES_MAX), "{name}");
            if count == 0 {
                continue;
            }
            assert_eq!(meshes.len(), 1, "{name}");
            let gaps: Vec<usize> = rays
                .windows(2)
                .map(|pair| pair[1] - pair[0])
                .chain([count - rays[rays.len() - 1] + rays[0]])
                .collect();
            let (lo, hi) = (gaps.iter().min().unwrap(), gaps.iter().max().unwrap());
            assert!(hi - lo <= 1, "{name}: uneven gaps {lo}..{hi}");
            built += 1;
        }
        assert!(built > 0, "a record bursts with a field");
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
    /// Damaged frame at [`timeline::DRIFT_DEG_S`] from the record's hold ([`pace`]). Two seconds
    /// on, every record's eye has turned that far about the centre at its distance and height, the
    /// tank still in the field; under reduce motion it has not moved.
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
                    pace(plan, CornerKind::Damaged).hold_from,
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
            CornerKind::Damaged => pace(plan, kind).hold_from + spin_deg / timeline::DRIFT_DEG_S,
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
        animated(&mut app);
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

    /// The real [`animate`] after [`rebuild`] on a [`staged`] app, with what it reads beside the
    /// stage: the knobs, the stage's gizmo group (configured without the render side that draws
    /// it), the inset camera and the round's model.
    fn animated(app: &mut App) {
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
            .insert_resource(Assets::<Image>::default())
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

    /// Every record's fields as the real [`rebuild`] draws them, in every frame, the corner's and
    /// the kill cam's lesson on the world alike: one volume, blended, on a box that holds every
    /// field out to its longest reach, reading the record's field maps from their atlas
    /// ([`field::atlas`]); its bands parted at the shipped thresholds and coloured from
    /// the band law alone — the knockout band the field teal, the reduced band the deep teal, both
    /// more blue than red and off the state scale, the knockout band the brighter — at one opacity
    /// across every record, so the colour says the band of the fields' summed intensity and
    /// nothing else. Outside the slice the volume is uncut; in the slice it is cut by the slice
    /// plane. Where its frame draws the body behind, it is bounded by its viewer's part mask
    /// ([`the_x_ray_draws_the_body_behind`]). At least one record throws a field in each. A box
    /// short of a field's reach clips that field's far side; a volume per field draws an overlap
    /// twice, in neither field's band.
    #[test]
    fn the_fields_are_one_volume_in_their_bands() {
        use super::super::potency::Band;
        let library = Library::load();
        let asset = TankAsset::load(&library.vehicle).unwrap();
        let (mut looked, mut sliced) = (0, 0);
        let mut opacities: Vec<f32> = Vec::new();
        let potency = Potency::shipped();
        let luminance = |c: Vec3| c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        let colour = |band: Band| LinearRgba::from(band.colour().unwrap()).to_vec3();
        let (knockout, reduced) = (colour(Band::Knockout), colour(Band::Reduced));
        for c in [knockout, reduced] {
            assert!(c.z > c.x && !on_state_scale(c), "{c}");
        }
        assert!(luminance(reduced) < luminance(knockout));
        for (index, record) in library.records.iter().enumerate() {
            let kind = record.plan.kind;
            if record.plan.cones.is_empty() || kind == CornerKind::Ricochet {
                continue;
            }
            let atlas = field::atlas(record.plan.maps()).data;
            for (kind, surface) in [
                (kind, super::super::Surface::Corner),
                (CornerKind::Glass, super::super::Surface::World),
            ] {
                let (mut app, _) = staged_on(&library, &asset, index, kind, Vec3::X, surface);
                let world = app.world_mut();
                let fields: Vec<(Handle<Mesh>, Transform, Handle<LookMaterial>)> = world
                    .query_filtered::<(&Mesh3d, &Transform, &MeshMaterial3d<LookMaterial>), With<FieldVolume>>()
                    .iter(world)
                    .map(|(mesh, pose, m)| (mesh.0.clone(), *pose, m.0.clone()))
                    .collect();
                assert_eq!(fields.len(), 1, "{} {surface:?}", record.stem);
                let slice = kind == CornerKind::Slice;
                let cut = world.resource::<Stage>().plane;
                assert_eq!(cut.is_some(), slice, "{} {surface:?}", record.stem);
                let maps = record.plan.maps();
                assert_eq!(maps.len(), record.plan.cones.len(), "{}", record.stem);
                let looks = world.resource::<Assets<LookMaterial>>();
                let meshes = world.resource::<Assets<Mesh>>();
                let images = world.resource::<Assets<Image>>();
                for (mesh, pose, handle) in &fields {
                    use bevy::camera::primitives::MeshAabb;
                    let aabb = meshes.get(mesh).and_then(|m| m.compute_aabb()).unwrap();
                    let (lo, hi) = (
                        pose.transform_point(Vec3::from(aabb.min())),
                        pose.transform_point(Vec3::from(aabb.max())),
                    );
                    for (cone, map) in record.plan.cones.iter().zip(maps.iter()) {
                        let reach = Vec3::splat(map.longest);
                        assert!(
                            (cone.origin - reach).cmpge(lo - 1e-4).all()
                                && (cone.origin + reach).cmple(hi + 1e-4).all(),
                            "{}: a field reaches past its volume's box",
                            record.stem
                        );
                    }
                    let m = looks.get(handle).unwrap();
                    assert!(
                        m.is_volume() && m.blend == Blend::Blended,
                        "{}",
                        record.stem
                    );
                    let read = m.field_map.as_ref().and_then(|map| images.get(map));
                    assert!(
                        read.is_some_and(|image| image.data == atlas),
                        "{} {surface:?}: the volume reads another atlas",
                        record.stem
                    );
                    assert_eq!(m.params.bands, potency.lanes(), "{}", record.stem);
                    assert_eq!(m.params.knockout.truncate(), knockout, "{}", record.stem);
                    assert_eq!(m.params.reduced.truncate(), reduced, "{}", record.stem);
                    match cut {
                        Some((normal, point)) => {
                            assert_eq!(m.params.mode.y, 1.0, "{}", record.stem);
                            assert_eq!(m.params.plane, normal.extend(normal.dot(point)));
                        }
                        None => assert_eq!(m.params.mode.y, 0.0, "{}", record.stem),
                    }
                    let bounded = look::body_behind(kind) as u8 as f32;
                    assert_eq!(m.params.volume.w, bounded, "{}", record.stem);
                    opacities.push(m.params.volume.x);
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
            opacities.iter().all(|o| *o == opacities[0]),
            "a volume's opacity varies across the records: {opacities:?}"
        );
    }

    /// A field is drawn as its volume alone, so a surface it meets keeps its own look: once the
    /// real [`animate`] has played a record to its hold, the volume holds the fields' lanes and
    /// maps, and every other look material on the stage (the components' base, the state fills,
    /// the masks' material) holds no field lane and no map. Every record that throws a field, in
    /// each frame that draws look surfaces beside a field: the glass frame and the outside view. A
    /// field painted on the surfaces it meets fails here.
    #[test]
    fn only_the_volume_reads_the_fields() {
        let library = Library::load();
        let asset = TankAsset::load(&library.vehicle).unwrap();
        let mut surfaces = 0;
        for (index, record) in library.records.iter().enumerate() {
            if record.plan.cones.is_empty() || record.plan.kind == CornerKind::Ricochet {
                continue;
            }
            for kind in [CornerKind::Glass, CornerKind::Damaged] {
                let at = format!("{} {kind:?}", record.stem);
                let (mut app, _) = staged(&library, &asset, index, kind, Vec3::ZERO);
                animated(&mut app);
                let hold = pace(&record.plan, kind).hold_from;
                app.world_mut()
                    .resource_mut::<Corner>()
                    .showing
                    .as_mut()
                    .expect("a frame on show")
                    .c = hold;
                app.update();
                let world = app.world_mut();
                let volume = world
                    .query_filtered::<&MeshMaterial3d<LookMaterial>, With<FieldVolume>>()
                    .single(world)
                    .expect("one volume")
                    .0
                    .id();
                for (id, m) in world.resource::<Assets<LookMaterial>>().iter() {
                    if id == volume {
                        assert!(m.params.fields.x > 0.0 && m.field_map.is_some(), "{at}");
                        continue;
                    }
                    let reads = m.params.fields.x > 0.0
                        || m.params.prints.iter().any(|lane| *lane != Vec4::ZERO)
                        || m.field_map.is_some();
                    assert!(!reads, "{at}: a look surface reads the fields");
                    surfaces += 1;
                }
            }
        }
        assert!(surfaces > 0, "no look surface beside a field");
    }

    /// One body mesh per body tier beside `app`'s staged components, all wearing the components'
    /// own material, and the stage built again over them. Fixture cubes: the draw order reads a
    /// mesh's tier, never its shape.
    fn with_body(app: &mut App) -> Vec<Entity> {
        use super::super::view::Tier;
        let world = app.world_mut();
        let cube = world
            .resource_mut::<Assets<Mesh>>()
            .add(Cuboid::new(1.0, 1.0, 1.0));
        let body = [Tier::Armour, Tier::External, Tier::Detail]
            .map(|tier| {
                world
                    .spawn((
                        ViewMesh {
                            root: ViewRoot::Stage,
                            names: Vec::new(),
                            original: Handle::default(),
                            tier,
                        },
                        Mesh3d(cube.clone()),
                        Transform::IDENTITY,
                    ))
                    .id()
            })
            .to_vec();
        world.resource_mut::<Stage>().invalidate();
        app.update();
        body
    }

    /// The mask channels `entity`'s children are copied into, each with its copy's material.
    fn mask_copies(world: &World, entity: Entity) -> Vec<(VisualScope, Handle<LookMaterial>)> {
        let masks = [
            VisualScope::PEN_CAM_HULL_MASK,
            VisualScope::PEN_CAM_PART_MASK,
        ];
        world
            .get::<Children>(entity)
            .into_iter()
            .flat_map(|children| children.iter())
            .filter_map(|child| {
                let scope = *world.get::<VisualScope>(child)?;
                let material = world.get::<MeshMaterial3d<LookMaterial>>(child)?;
                masks.contains(&scope).then(|| (scope, material.0.clone()))
            })
            .collect()
    }

    /// Every frame that draws the body behind puts each of the tank's meshes on its tier's side of
    /// the draw order as the real [`rebuild`] dresses it, in the corner's glass, the victim's, the
    /// kill cam's lesson on the world and the slice: a body mesh (main armour, external armour,
    /// detail) wears the body's glass, discarded where the part mask holds a component, and is
    /// copied into the hull mask alone, or in the slice wears the tank's own material, discarded
    /// likewise on the cut's kept side, and is copied into no mask; a component wears the opaque
    /// grey tested against the part mask, or in the slice the tank's own material, and is copied
    /// into the part mask alone, in the slice with the cut. Every volume is bounded by its viewer's
    /// part mask, and every outline sorts after the body's glass and before the volumes. The x-ray
    /// relies on it: a body mesh on the scene's side covers the components with glass, a component
    /// on the body's side bounds no field and hides behind the glass, and a volume without the
    /// bound draws over a component in front of it.
    #[test]
    fn the_x_ray_draws_the_body_behind() {
        use super::super::Surface;
        let library = Library::load();
        let asset = TankAsset::load(&library.vehicle).unwrap();
        // Records checked, and outlines checked, per frame; the order is per mesh, never per
        // record, so a few records cover it.
        let (mut checked, mut outlined) = ([0; 4], 0);
        for (index, record) in library.records.iter().enumerate() {
            if record.plan.cones.is_empty() || !record.plan.boxes.iter().any(|b| b.struck()) {
                continue;
            }
            if checked.iter().all(|n| *n >= 3) {
                break;
            }
            for (frame, (kind, surface)) in [
                (CornerKind::Glass, Surface::Corner),
                (CornerKind::Victim, Surface::Corner),
                (CornerKind::Glass, Surface::World),
                (CornerKind::Slice, Surface::Corner),
            ]
            .into_iter()
            .enumerate()
            {
                let (mut app, placed) = staged_on(&library, &asset, index, kind, Vec3::X, surface);
                let body = with_body(&mut app);
                let world = app.world_mut();
                let (masks, plane) = {
                    let stage = world.resource::<Stage>();
                    (stage.masks.clone(), stage.plane)
                };
                let at = |what: &str| format!("{} {kind:?} {surface:?}: {what}", record.stem);
                let looks = world.resource::<Assets<LookMaterial>>();
                let slices = world.resource::<Assets<SliceMaterial>>();
                let worn = |entity: Entity| {
                    world
                        .get::<MeshMaterial3d<LookMaterial>>(entity)
                        .and_then(|m| looks.get(&m.0))
                };
                let sliced = |entity: Entity| {
                    world
                        .get::<MeshMaterial3d<SliceMaterial>>(entity)
                        .and_then(|m| slices.get(&m.0))
                };
                let meshes = body
                    .iter()
                    .map(|entity| (*entity, Layer::Body, "body".to_owned()))
                    .chain(
                        placed
                            .iter()
                            .map(|(entity, name, _)| (*entity, Layer::Scene, name.clone())),
                    );
                for (entity, layer, name) in meshes {
                    let copies = mask_copies(world, entity);
                    let into = match layer {
                        Layer::Body => VisualScope::PEN_CAM_HULL_MASK,
                        Layer::Scene => VisualScope::PEN_CAM_PART_MASK,
                    };
                    if kind == CornerKind::Slice {
                        let slice = sliced(entity).unwrap_or_else(|| panic!("{}", at(&name)));
                        let behind = (layer == Layer::Body) as u8 as f32;
                        assert_eq!(slice.extension.params.params.w, behind, "{}", at(&name));
                        assert_eq!(slice.extension.part_mask.as_ref(), Some(&masks.part));
                        match layer {
                            Layer::Body => assert!(copies.is_empty(), "{}", at(&name)),
                            Layer::Scene => {
                                assert_eq!(copies.len(), 1, "{}", at(&name));
                                assert_eq!(copies[0].0, into, "{}", at(&name));
                                let (normal, point) = plane.unwrap();
                                let copy = looks.get(&copies[0].1).unwrap();
                                assert_eq!(copy.params.mode.y, 1.0, "{}", at(&name));
                                assert_eq!(copy.params.plane, normal.extend(normal.dot(point)));
                            }
                        }
                    } else {
                        let m = worn(entity).unwrap_or_else(|| panic!("{}", at(&name)));
                        let (z, blend) = match layer {
                            Layer::Body => (0.0, Blend::Blended),
                            Layer::Scene => (1.0, Blend::Opaque),
                        };
                        assert!(!m.is_volume(), "{}", at(&name));
                        assert_eq!((m.params.mode.z, m.blend), (z, blend), "{}", at(&name));
                        assert_eq!(m.part_mask.as_ref(), Some(&masks.part), "{}", at(&name));
                        assert_eq!(copies.len(), 1, "{}", at(&name));
                        assert_eq!(copies[0].0, into, "{}", at(&name));
                    }
                }
                let volumes: Vec<Handle<LookMaterial>> = world
                    .query_filtered::<&MeshMaterial3d<LookMaterial>, With<FieldVolume>>()
                    .iter(world)
                    .map(|m| m.0.clone())
                    .collect();
                let looks = world.resource::<Assets<LookMaterial>>();
                assert!(!volumes.is_empty(), "{}", at("volumes"));
                for handle in &volumes {
                    let m = looks.get(handle).unwrap();
                    assert_eq!(m.params.volume.w, 1.0, "{}", at("volume"));
                    assert_eq!(m.part_mask.as_ref(), Some(&masks.part), "{}", at("volume"));
                    assert_eq!(m.depth_bias(), look::VOLUME_SORT_BIAS);
                }
                let outlines: Vec<Handle<OutlineMaterial>> = world
                    .query::<&MeshMaterial3d<OutlineMaterial>>()
                    .iter(world)
                    .map(|m| m.0.clone())
                    .collect();
                let store = world.resource::<Assets<OutlineMaterial>>();
                for handle in &outlines {
                    let outline = store.get(handle).unwrap();
                    assert_eq!(outline.alpha_mode(), AlphaMode::Blend, "{}", at("outline"));
                    let bias = outline.depth_bias();
                    assert!(
                        bias > 0.0 && bias < look::VOLUME_SORT_BIAS,
                        "{}",
                        at("outline")
                    );
                }
                checked[frame] += 1;
                outlined += outlines.len();
            }
        }
        assert!(
            checked.iter().all(|n| *n > 0) && outlined > 0,
            "{checked:?}"
        );
    }

    /// A stand-in for the round's model beside `app`'s stage, as a glTF instance has it: two
    /// meshes down a node chain under the shell, each in its own standard material, and the stage
    /// built again. Fixture cuboids: the order reads that a mesh is the round's, never its shape.
    /// Returns each mesh with its material.
    fn with_round(app: &mut App) -> Vec<(Entity, Handle<StandardMaterial>)> {
        let world = app.world_mut();
        let cuboid = world
            .resource_mut::<Assets<Mesh>>()
            .add(Cuboid::new(0.1, 0.1, 0.3));
        let shell = world
            .spawn((Transform::IDENTITY, Visibility::Inherited, StageShell))
            .id();
        let mut parent = world.spawn((Transform::IDENTITY, ChildOf(shell))).id();
        let mut round = Vec::new();
        for z in [0.0, 0.2] {
            let material = world
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial::default());
            parent = world
                .spawn((
                    Mesh3d(cuboid.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::from_xyz(0.0, 0.0, z),
                    ChildOf(parent),
                ))
                .id();
            round.push((parent, material));
        }
        world.resource_mut::<Stage>().invalidate();
        app.update();
        round
    }

    /// The round's model is on the scene side of the draw order in every frame that draws the body
    /// behind, as the real [`rebuild`] dresses it, built twice: in the corner's glass, the
    /// victim's, the kill cam's lesson on the world and the slice, each of its meshes keeps its own
    /// material and carries exactly one mask copy of its own mesh, in the part mask, drawn with its
    /// viewer's masks' material uncut, the slice's included, where the round is drawn whole. The
    /// outside frames copy it into no mask. The corner, the lesson and the slice rely on it:
    /// without the copy a field behind the round draws over it and the body's glass covers it, a
    /// copy in the hull mask bounds no field, a cut copy leaves the round's near half unbounded,
    /// and a rebuild that copies the copies leaves strays in the part mask.
    #[test]
    fn the_round_is_on_the_scene_side() {
        use super::super::Surface;
        let library = Library::load();
        let asset = TankAsset::load(&library.vehicle).unwrap();
        let index = library
            .records
            .iter()
            .position(|record| !record.plan.cones.is_empty())
            .expect("a record with a spall cone");
        for (kind, surface) in [
            (CornerKind::Glass, Surface::Corner),
            (CornerKind::Victim, Surface::Corner),
            (CornerKind::Glass, Surface::World),
            (CornerKind::Slice, Surface::Corner),
            (CornerKind::Damaged, Surface::Corner),
            (CornerKind::Ricochet, Surface::Corner),
        ] {
            let (mut app, _) = staged_on(&library, &asset, index, kind, Vec3::X, surface);
            let round = with_round(&mut app);
            app.world_mut().resource_mut::<Stage>().invalidate();
            app.update();
            let world = app.world_mut();
            let at = format!("{kind:?} {surface:?}");
            let cuboid = world.get::<Mesh3d>(round[0].0).unwrap().0.clone();
            let copied = world
                .query_filtered::<&Mesh3d, With<MeshMaterial3d<LookMaterial>>>()
                .iter(world)
                .filter(|mesh| mesh.0 == cuboid)
                .count();
            let behind = look::body_behind(kind);
            assert_eq!(copied, if behind { round.len() } else { 0 }, "{at}");
            let uncut = look::hull_mask(&world.resource::<Stage>().masks);
            let looks = world.resource::<Assets<LookMaterial>>();
            for (mesh, material) in &round {
                let own = world.get::<MeshMaterial3d<StandardMaterial>>(*mesh);
                assert_eq!(own.map(|m| &m.0), Some(material), "{at}: its own material");
                let copies = mask_copies(world, *mesh);
                if !behind {
                    assert!(copies.is_empty(), "{at}");
                    continue;
                }
                assert_eq!(copies.len(), 1, "{at}");
                assert_eq!(copies[0].0, VisualScope::PEN_CAM_PART_MASK, "{at}");
                let copy = looks.get(&copies[0].1).unwrap();
                assert_eq!(copy.params.mode, uncut.params.mode, "{at}: uncut");
                assert_eq!(copy.hull_mask, uncut.hull_mask, "{at}");
                assert_eq!(copy.part_mask, uncut.part_mask, "{at}");
            }
        }
    }

    /// The round's model lands after the stage is built, a glTF scene instancing late: the real
    /// [`spawn_inset`]'s round, instanced by the real spawner, marks the stage for a rebuild, so
    /// its meshes get their mask copies. The corner and the lesson rely on it: a round landing
    /// after its record went on show would bound no field until the record or frame changed.
    #[test]
    fn the_round_landing_rebuilds_the_stage() {
        use bevy::world_serialization::WorldSerializationPlugin;
        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            WorldSerializationPlugin,
        ))
        .init_asset::<Image>()
        .init_asset::<StandardMaterial>()
        .init_resource::<Stage>()
        .add_systems(Startup, spawn);
        app.update();
        let world = app.world_mut();
        let shell = world
            .query_filtered::<Entity, With<StageShell>>()
            .single(world)
            .expect("the inset's round");
        let mut model = World::new();
        model.spawn(Transform::IDENTITY);
        let model = world
            .resource_mut::<Assets<WorldAsset>>()
            .add(WorldAsset::new(model));
        world.entity_mut(shell).insert(WorldAssetRoot(model));
        world.resource_mut::<Stage>().stale = false;
        let mut frames = 0;
        while !app.world().resource::<Stage>().stale && frames < 8 {
            app.update();
            frames += 1;
        }
        assert!(
            app.world().resource::<Stage>().stale,
            "after {frames} frames"
        );
        let world = app.world();
        assert!(
            world
                .get::<Children>(shell)
                .is_some_and(|children| !children.is_empty()),
            "the model is instanced under the round"
        );
    }

    /// Fixture data: the playlist's records with every component's pool doubled, so each stood at
    /// half its pool before the shot, and the first component no dose names left with no hit
    /// points at the shot, lost before it. No recorded scenario carries damage from before its
    /// shot.
    fn with_prior_damage() -> Library {
        let reports = Library::reports()
            .into_iter()
            .map(|(stem, mut report)| {
                for component in &mut report.components {
                    component.count *= 2;
                }
                let undosed = report
                    .components
                    .iter()
                    .position(|component| !report.doses.iter().any(|d| d.volume == component.name));
                if let Some(index) = undosed {
                    report.components[index].hp = 0;
                }
                (stem, report)
            })
            .collect();
        Library::of(reports)
    }

    /// Component `index` of `plan` as the real [`Playing::fill`] leaves it in frame `kind` at
    /// corner time `c`, over the material [`look::component`] builds for it: its fill's lanes and
    /// its outline's fade while shown, `None` while hidden or where the frame does not draw it.
    fn fill_at(
        plan: &Plan,
        kind: CornerKind,
        index: usize,
        c: f32,
    ) -> Option<(look::LookParams, f32)> {
        let masks = MaskSet::default();
        let mut looks = Assets::<LookMaterial>::default();
        let mut outlines = Assets::<OutlineMaterial>::default();
        let look = MeshMaterial3d(looks.add(look::component(&plan.boxes[index], kind, &masks)?));
        let outline = MeshMaterial3d(outlines.add(OutlineMaterial::new(
            timeline::STATE_RED.into(),
            look::STATE_OUTLINE_PX,
            Vec3::ZERO,
        )));
        let mut world = World::new();
        let entity = world.spawn(Visibility::Hidden).id();
        let mut visibility = world.get_mut::<Visibility>(entity).unwrap();
        Playing::new(plan, kind, c, false).fill(
            &BoxTint { index },
            Some(&outline),
            Some(&look),
            &mut visibility,
            None,
            &mut outlines,
            &mut looks,
        );
        (*visibility == Visibility::Inherited).then(|| {
            (
                looks.get(&look.0).unwrap().params,
                outlines.get(&outline.0).unwrap().params.color.w,
            )
        })
    }

    /// A state fill's lanes, read back as the place on the state scale they draw: out of action
    /// where the hatch is on, else reduced to the level.
    fn read_condition(params: &look::LookParams) -> Condition {
        if params.mode.w > 0.5 {
            Condition::Dead
        } else {
            Condition::Damaged(params.state.w)
        }
    }

    /// A state scale colour, opaque, as a fill's lanes carry it.
    fn lanes(color: Color) -> Vec4 {
        let c = LinearRgba::from(color);
        Vec4::new(c.red, c.green, c.blue, 1.0)
    }

    /// Every component the real [`rebuild`] draws, in the shooter's glass and outside view and in
    /// the victim's glass, played by the real [`Playing::fill`] to the record's settle, is on the
    /// one state scale under a red outline: a reduced one opaque red to the share of its whole pool
    /// gone, prior damage included, over the untouched grey; one out of action charcoal and
    /// hatched. The shooter's frames draw exactly the components the shot took HP from, the
    /// victim's every one with any of its pool gone. Over the records with damage before the shot
    /// (fixture data), at least one of each in each view. The shooter relies on the frame never
    /// showing earlier damage of a component the shot missed, and every viewer on the fill reading
    /// the whole pool: this shot's share of what remained reads a component that had lost half its
    /// pool and lost a quarter more as half gone.
    #[test]
    fn a_component_reads_the_share_of_its_pool_gone() {
        use bevy::ecs::system::RunSystemOnce;
        let library = with_prior_damage();
        let asset = TankAsset::load(&library.vehicle).unwrap();
        let (mut reduced, mut out) = ([0; 2], [0; 2]);
        for (index, record) in library.records.iter().enumerate() {
            let shapes = asset.shapes(&record.report.servo_deg);
            for kind in [CornerKind::Glass, CornerKind::Damaged, CornerKind::Victim] {
                let victim = (kind == CornerKind::Victim) as usize;
                let (mut app, _) = staged(&library, &asset, index, kind, Vec3::X);
                let plan = record.plan.clone();
                let settle = pace(&plan, kind).settle;
                app.world_mut()
                    .run_system_once(
                        move |mut boxes: Query<(
                            &BoxTint,
                            Option<&MeshMaterial3d<OutlineMaterial>>,
                            Option<&MeshMaterial3d<LookMaterial>>,
                            &mut Visibility,
                        )>,
                              mut outlines: ResMut<Assets<OutlineMaterial>>,
                              mut looks: ResMut<Assets<LookMaterial>>| {
                            let playing = Playing::new(&plan, kind, settle, false);
                            for (tint, outline, look, mut visibility) in &mut boxes {
                                playing.fill(
                                    tint,
                                    outline,
                                    look,
                                    &mut visibility,
                                    None,
                                    &mut outlines,
                                    &mut looks,
                                );
                            }
                        },
                    )
                    .unwrap();
                let world = app.world_mut();
                let outlined: Vec<(usize, Vec4)> = world
                    .query::<(&BoxTint, &MeshMaterial3d<OutlineMaterial>)>()
                    .iter(world)
                    .map(|(tint, m)| {
                        let outlines = world.resource::<Assets<OutlineMaterial>>();
                        (tint.index, outlines.get(&m.0).unwrap().params.color)
                    })
                    .collect();
                let mut tints =
                    world.query::<(&BoxTint, &MeshMaterial3d<LookMaterial>, &Visibility)>();
                let looks = world.resource::<Assets<LookMaterial>>();
                let mut drawn = Vec::new();
                for (tint, material, visibility) in tints.iter(world) {
                    let b = &record.plan.boxes[tint.index];
                    let m = looks.get(&material.0).unwrap();
                    let name = format!("{} {kind:?} {}", record.stem, b.name);
                    drawn.push(tint.index);
                    assert_eq!(*visibility, Visibility::Inherited, "{name}: hidden");
                    assert!(
                        outlined.iter().any(|(index, color)| *index == tint.index
                            && *color == lanes(timeline::STATE_RED)),
                        "{name}: no whole red outline"
                    );
                    assert_eq!(m.blend, Blend::Opaque, "{name}: not opaque");
                    assert_eq!(m.params.rest, lanes(timeline::STATE_UNTOUCHED), "{name}");
                    let share = b.pool_lost();
                    if b.lost || share >= 1.0 {
                        assert_eq!(m.params.color, lanes(timeline::STATE_DEAD), "{name}");
                        assert_eq!(
                            m.params.state.w, 1.0,
                            "{name}: an out-of-action fill is whole"
                        );
                        assert!(m.params.mode.w > 0.5, "{name}: out of action and unhatched");
                        out[victim] += 1;
                        continue;
                    }
                    assert_eq!(m.params.color, lanes(timeline::STATE_RED), "{name}");
                    assert_eq!(m.params.state.w, share, "{name}");
                    assert!(m.params.mode.w < 0.5, "{name}: reduced and hatched");
                    reduced[victim] += 1;
                }
                drawn.sort_unstable();
                drawn.dedup();
                let want: Vec<usize> = (0..record.plan.boxes.len())
                    .filter(|&i| {
                        let b = &record.plan.boxes[i];
                        let shown = if victim == 1 {
                            b.lost || b.pool_lost() > 0.0
                        } else {
                            b.struck()
                        };
                        shown && shapes.get(&b.name).is_some()
                    })
                    .collect();
                assert_eq!(drawn, want, "{} {kind:?}", record.stem);
            }
        }
        assert!(
            reduced.iter().all(|n| *n > 0) && out.iter().all(|n| *n > 0),
            "{reduced:?} reduced, {out:?} out of action"
        );
    }

    /// A component the shot took HP from, and that had lost some before it, reads its share
    /// before the shot from the first frame until the picture reaches it, never less from then
    /// on, and its state after the shot once its fill completes, outline whole throughout, in
    /// every frame that draws it (damage before the shot: fixture data). Every viewer relies on
    /// it: a fill that rises from nothing shows a reduced component untouched until it is struck.
    #[test]
    fn a_struck_component_rises_from_its_state_before_the_shot() {
        let library = with_prior_damage();
        let mut rose = 0;
        for record in &library.records {
            let plan = &*record.plan;
            for kind in [CornerKind::Glass, CornerKind::Damaged, CornerKind::Victim] {
                let settle = pace(plan, kind).settle;
                for (index, b) in plan.boxes.iter().enumerate() {
                    if !b.struck() {
                        continue;
                    }
                    let name = format!("{} {kind:?} {}", record.stem, b.name);
                    let before = Condition::Damaged(b.pool_lost_before());
                    let start = box_start(plan, kind, b, false);
                    let at = |c: f32| {
                        let (params, outline) = fill_at(plan, kind, index, c)
                            .unwrap_or_else(|| panic!("{name}: hidden at {c}"));
                        assert_eq!(outline, 1.0, "{name}: a faded outline at {c}");
                        read_condition(&params)
                    };
                    for c in [0.0, 0.5 * start, start] {
                        assert_eq!(at(c), before, "{name} at {c}, reached at {start}");
                    }
                    for k in 1..=16 {
                        let c = start + timeline::BOX_FILL_S * k as f32 / 16.0;
                        let now = at(c);
                        assert!(
                            now.fill() >= before.fill(),
                            "{name} at {c}: {now:?} below {before:?}"
                        );
                    }
                    let after = Condition::of(b.lost, b.pool_lost()).expect("struck");
                    assert_eq!(at(settle), after, "{name} settled");
                    rose += 1;
                }
            }
        }
        assert!(rose > 0, "a struck component");
    }

    /// In the victim's glass, a component this shot took no HP from shows its state from the
    /// first frame and holds it, outline whole, through the settle: red to its share of the pool
    /// gone, or out of action when it was lost before the shot (damage before the shot: fixture
    /// data). The victim relies on it: a fill that rises at the open reads as this shot's work.
    #[test]
    fn a_component_the_shot_missed_holds_its_state() {
        let library = with_prior_damage();
        let kind = CornerKind::Victim;
        let (mut reduced, mut out) = (0, 0);
        for record in &library.records {
            let plan = &*record.plan;
            let settle = pace(plan, kind).settle;
            for (index, b) in plan.boxes.iter().enumerate() {
                if b.struck() {
                    continue;
                }
                let Some(state) = Condition::of(b.lost, b.pool_lost()) else {
                    continue;
                };
                let name = format!("{} {}", record.stem, b.name);
                for k in 0..=16 {
                    let c = settle * k as f32 / 16.0;
                    let (params, outline) = fill_at(plan, kind, index, c)
                        .unwrap_or_else(|| panic!("{name}: hidden at {c}"));
                    assert_eq!(read_condition(&params), state, "{name} at {c}");
                    assert_eq!(outline, 1.0, "{name}: a faded outline at {c}");
                }
                match state {
                    Condition::Damaged(_) => reduced += 1,
                    Condition::Dead => out += 1,
                }
            }
        }
        assert!(
            reduced > 0 && out > 0,
            "{reduced} reduced, {out} out of action"
        );
    }

    /// A component this shot empties reads red and unhatched, its level never falling, until its
    /// fill completes, and out of action (charcoal, hatched, whole) from that moment on, in every
    /// frame that draws it. Every viewer relies on it to see the rise before the knockout: out of
    /// action from the start hides how far the shot took it.
    #[test]
    fn a_component_the_shot_empties_is_knocked_out_when_its_rise_completes() {
        let library = with_prior_damage();
        let mut emptied = 0;
        for record in &library.records {
            let plan = &*record.plan;
            for kind in [CornerKind::Glass, CornerKind::Damaged, CornerKind::Victim] {
                for (index, b) in plan.boxes.iter().enumerate() {
                    if !(b.struck() && b.lost) {
                        continue;
                    }
                    let name = format!("{} {kind:?} {}", record.stem, b.name);
                    let start = box_start(plan, kind, b, false);
                    let done = start + timeline::BOX_FILL_S;
                    let at = |c: f32| {
                        fill_at(plan, kind, index, c)
                            .unwrap_or_else(|| panic!("{name}: hidden at {c}"))
                            .0
                    };
                    let mut level = 0.0;
                    for c in [0.0, start, start + 0.1, start + 0.2, done - 0.01] {
                        let params = at(c);
                        assert_eq!(params.color, lanes(timeline::STATE_RED), "{name} at {c}");
                        assert!(params.mode.w < 0.5, "{name}: hatched at {c}");
                        assert!(params.state.w >= level, "{name}: falls at {c}");
                        level = params.state.w;
                    }
                    assert!(level < 1.0, "{name}: whole before its rise completes");
                    for c in [done, done + 1.0] {
                        let params = at(c);
                        assert_eq!(params.color, lanes(timeline::STATE_DEAD), "{name} at {c}");
                        assert!(params.mode.w > 0.5, "{name}: unhatched at {c}");
                        assert_eq!(params.state.w, 1.0, "{name} at {c}");
                    }
                    emptied += 1;
                }
            }
        }
        assert!(emptied > 0, "a component the shot empties");
    }

    /// The on-screen speed (m per corner-second) of cone `index`'s front, read across the middle
    /// of its run.
    fn front_speed(plan: &Plan, kind: CornerKind, index: usize) -> f32 {
        let start = cone_start(plan, kind, index, false);
        let cone = &plan.cones[index];
        let run = cone.run_time(cone.max_reach()) * timeline::REPLAY_SLOWDOWN;
        let (a, b) = (start + 0.25 * run, start + 0.75 * run);
        (front_radius(plan, kind, index, b) - front_radius(plan, kind, index, a)) / (b - a)
    }

    /// For a spall cone thrown at record time `t`: the record's speed (m/s) on the path leg
    /// leaving the node nearest `t` in time, where the crossing that threw it ends, and the speed
    /// of the leg `t` itself falls on. Read off the path's times alone, never the cone's origin.
    fn exit_speed(report: &PenetrationReport, t: f32) -> (f32, f32) {
        let path = &report.path;
        let gap = |i: usize| (path[i].t - t).abs();
        let at = (0..path.len())
            .min_by(|&a, &b| gap(a).total_cmp(&gap(b)))
            .expect("a recorded path");
        let speed = |a: &crate::damage_cam::PathPoint, b: &crate::damage_cam::PathPoint| {
            Vec3::from(a.point).distance(Vec3::from(b.point)) / (b.t - a.t)
        };
        let leaving = path[at + 1..]
            .iter()
            .find(|next| Vec3::from(next.point).distance(Vec3::from(path[at].point)) > 1e-6)
            .map(|next| speed(&path[at], next))
            .expect("a timed leg leaves the node a spall cone is thrown at");
        let holding = path
            .windows(2)
            .find(|pair| pair[0].t <= t && t < pair[1].t)
            .map_or(leaving, |pair| speed(&pair[0], &pair[1]));
        (leaving, holding)
    }

    /// Every field's front sets off at its own record time and runs at its own speed over
    /// [`timeline::REPLAY_SLOWDOWN`], read off the record apart from the plan: a spall cone's at
    /// the round's speed on the path leg leaving the plate that threw it, a burst's field's at the
    /// burst's sideways speed `v_s`; neither is the line's pace. A component a front doses starts
    /// its fill when that front reaches it. Every spall cone is checked, its origin on the path or
    /// off it. The library holds a spall cone whose exit speed is under half its contact speed, one
    /// whose origin projects inside the crossing that threw it (its own time falls on the in-plate
    /// leg), and a burst whose `v_s` is apart from its carried speed, so neither the contact speed,
    /// the leg the origin projects onto nor the carried speed passes.
    #[test]
    fn each_front_runs_at_its_own_speed() {
        let library = Library::load();
        let (mut spall, mut inside, mut burst) = (0, 0, 0);
        for record in &library.records {
            let (plan, report) = (&record.plan, &record.report);
            let name = record.stem;
            for kind in [CornerKind::Glass, plan.kind] {
                for (index, cone) in plan.cones.iter().enumerate() {
                    let start = cone_start(plan, kind, index, false);
                    assert!(
                        front_radius(plan, kind, index, start).abs() < 1e-5,
                        "{name}"
                    );
                    assert!(
                        front_radius(plan, kind, index, start - 0.01) < 0.0,
                        "{name}"
                    );
                    let want = match report.spall.get(index) {
                        Some(c) => {
                            assert_eq!(cone.t, c.t, "{name} cone {index}");
                            let (exit, holding) = exit_speed(report, c.t);
                            if exit < 0.5 * report.projectile.speed_mps {
                                spall += 1;
                            }
                            if (holding - exit).abs() > 0.05 * exit {
                                inside += 1;
                            }
                            exit
                        }
                        None => {
                            let b = report
                                .burst
                                .as_ref()
                                .expect("a cone past the spall is the burst's");
                            assert_eq!(cone.t, b.t, "{name} burst");
                            if (b.sideways_mps - b.speed_mps).abs() > 0.25 * b.sideways_mps {
                                burst += 1;
                            }
                            b.sideways_mps
                        }
                    };
                    if cone.max_reach() <= 0.0 {
                        continue;
                    }
                    let shown = front_speed(plan, kind, index);
                    let scaled = want / timeline::REPLAY_SLOWDOWN;
                    assert!(
                        (shown - scaled).abs() <= 1e-3 * scaled,
                        "{name} cone {index}: {shown} m per corner-s for {want} m/s"
                    );
                }
                for b in &plan.boxes {
                    if let Some(Reach::Cone { cone, distance }) = b.reached_at {
                        let at = box_start(plan, kind, b, false);
                        assert!(
                            (front_radius(plan, kind, cone, at) - distance).abs() < 1e-3,
                            "{name}: {}",
                            b.name
                        );
                    }
                }
            }
        }
        assert!(
            spall > 0,
            "a spall cone's exit speed is apart from its contact speed"
        );
        assert!(
            inside > 0,
            "a spall cone's origin projects inside its crossing"
        );
        assert!(burst > 0, "a burst's v_s is apart from its carried speed");
    }

    /// A component's fill starts the first moment the picture reaches it, in every frame, the
    /// victim's included: the earliest of the nose entering it where the record has the line
    /// enter, and each field that dosed it having run, from its own record time at its own speed,
    /// the reach of its shortest direction that dosed it. Every candidate is read off the record
    /// apart from the plan's choice. The corner's fill and the pace's settle start from it; a
    /// victim's fill that starts at the open shows this shot's damage before the round arrives. The library holds a component a later-listed field
    /// reaches first and one a front reaches before the nose enters it, so neither the first-listed
    /// dosing field nor the line taken first whenever it dosed passes.
    #[test]
    fn each_component_fills_from_its_earliest_arrival() {
        let library = Library::load();
        let (mut later_field, mut front_first) = (0, 0);
        for record in &library.records {
            let (plan, report) = (&record.plan, &record.report);
            let name = record.stem;
            for kind in [CornerKind::Glass, plan.kind, CornerKind::Victim] {
                for b in &plan.boxes {
                    let Some(dose) = report.doses.iter().find(|d| d.volume == b.name) else {
                        continue;
                    };
                    let by_line = dose.entered.filter(|_| dose.line > 0.0).map(|entered| {
                        let arc = plan.line.arc_of(Vec3::from(entered));
                        time_at_arc(plan, kind, arc, false)
                    });
                    let by_field: Vec<f32> = report
                        .fields()
                        .enumerate()
                        .filter_map(|(index, field)| {
                            let origin = Vec3::from(field.origin);
                            let dosed = |fragment: &&crate::damage_cam::Fragment| {
                                fragment
                                    .hits
                                    .iter()
                                    .any(|hit| hit.volume == b.name && hit.damage > 0.0)
                            };
                            let reach = field
                                .fragments
                                .iter()
                                .filter(dosed)
                                .map(|fragment| Vec3::from(fragment.end).distance(origin))
                                .reduce(f32::min)?;
                            let run = plan.cones[index].run_time(reach);
                            let from = cone_start(plan, kind, index, false);
                            Some(from + run * timeline::REPLAY_SLOWDOWN)
                        })
                        .collect();
                    let first_front = by_field.iter().copied().reduce(f32::min);
                    let Some(earliest) = by_line.into_iter().chain(first_front).reduce(f32::min)
                    else {
                        continue;
                    };
                    let start = box_start(plan, kind, b, false);
                    assert!(
                        (start - earliest).abs() < 1e-4,
                        "{name} {kind:?} {}: fills at {start}, reached at {earliest}",
                        b.name
                    );
                    if by_field
                        .first()
                        .is_some_and(|&listed| listed > earliest + 1e-3)
                        && by_line.is_none_or(|line| line > earliest + 1e-3)
                    {
                        later_field += 1;
                    }
                    if let (Some(line), Some(front)) = (by_line, first_front)
                        && front < line - 1e-3
                    {
                        front_first += 1;
                    }
                }
            }
        }
        assert!(
            later_field > 0,
            "a later-listed field reaches a component first"
        );
        assert!(
            front_first > 0,
            "a front reaches a component before the nose"
        );
    }

    /// The approach is a diagram choice, so its length never paces the record: cutting a record's
    /// approach to a quarter and to three quarters of the drawn lead moves when its fields are
    /// thrown, but not how each front grows from then nor when each dosed component's fill starts
    /// after it. Any line-length term in the pace fails here.
    #[test]
    fn the_approach_paces_no_front() {
        let library = Library::load();
        let asset = TankAsset::load(&library.vehicle).unwrap();
        let mut compared = 0;
        for record in &library.records {
            let base = &record.plan;
            if base.cones.is_empty() {
                continue;
            }
            let report = &record.report;
            let name = record.stem;
            assert_eq!(
                report.path[1].t, 0.0,
                "{name}: contact is the path's second node"
            );
            let contact = Vec3::from(report.path[1].point);
            let approach = contact - Vec3::from(report.path[0].point);
            let speed = approach.length() / (report.path[1].t - report.path[0].t);
            let lead = base.reach(LEAD_M);
            let cut = |share: f32| {
                let mut report = report.clone();
                let l = share * lead;
                let from = contact - approach.normalize() * l;
                report.path[0].point = from.to_array();
                report.path[0].t = -l / speed;
                report.origin = from.to_array();
                Plan::new(&report, &asset.shapes(&report.servo_deg))
            };
            let (short, long) = (cut(0.25), cut(0.75));
            for kind in [CornerKind::Glass, base.kind] {
                let from = |plan: &Plan, i: usize| cone_start(plan, kind, i, false);
                assert!(
                    from(&long, 0) - from(&short, 0) > 0.05,
                    "{name}: a longer approach throws later"
                );
                for index in 0..base.cones.len() {
                    for k in 0..=16 {
                        let x = 0.05 * k as f32;
                        let a = front_radius(&short, kind, index, from(&short, index) + x);
                        let b = front_radius(&long, kind, index, from(&long, index) + x);
                        assert!(
                            (a - b).abs() < 1e-3,
                            "{name} cone {index} at +{x}: {a} vs {b}"
                        );
                    }
                }
                for (bs, bl) in short.boxes.iter().zip(&long.boxes) {
                    if let Some(Reach::Cone { cone, .. }) = bs.reached_at {
                        let after = |plan: &Plan, b: &BoxPlan| {
                            box_start(plan, kind, b, false) - from(plan, cone)
                        };
                        let (a, b) = (after(&short, bs), after(&long, bl));
                        assert!((a - b).abs() < 1e-3, "{name} {}: {a} vs {b}", bs.name);
                    }
                }
            }
            compared += 1;
        }
        assert!(compared > 0, "a record throws a field");
    }

    /// A record's corner runs no shorter than its frame's minimum and holds its full hold beat and
    /// close after every element has arrived: `hold_from >= settle`, `duration >= settle + hold +
    /// close`. The hold key and the corner's end in the game, the demo and the x-ray read these.
    /// Both regimes occur: a record that settles late runs past the minimum, and the x-ray's
    /// record-less plan settles early and runs the minimum, so neither a fixed length nor a dropped
    /// minimum passes.
    #[test]
    fn the_corner_holds_after_the_record_settles() {
        let library = Library::load();
        let bare = Plan::bare(&TankShapes::default(), Vec::new());
        let plans = library
            .records
            .iter()
            .map(|r| (r.stem, &*r.plan))
            .chain([("bare", &bare)]);
        let (mut late, mut early) = (0, 0);
        for (name, plan) in plans {
            for kind in [
                CornerKind::Glass,
                CornerKind::Slice,
                CornerKind::Ricochet,
                CornerKind::Damaged,
                CornerKind::Victim,
            ] {
                let pace = pace(plan, kind);
                let eps = 1e-5;
                assert!(pace.settle >= pace.travel_end, "{name} {kind:?}");
                assert!(pace.hold_from >= pace.settle - eps, "{name} {kind:?}");
                assert!(
                    pace.hold_from >= kind.min_hold_from() - eps,
                    "{name} {kind:?}"
                );
                assert!(
                    pace.duration >= kind.min_duration() - eps,
                    "{name} {kind:?}"
                );
                assert!(
                    pace.duration >= pace.settle + kind.hold_s() + timeline::CLOSE_S - eps,
                    "{name} {kind:?}"
                );
                if pace.duration > kind.min_duration() + eps {
                    late += 1;
                } else if pace.settle < kind.min_hold_from() {
                    early += 1;
                }
            }
        }
        assert!(late > 0 && early > 0, "late {late}, early {early}");
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
