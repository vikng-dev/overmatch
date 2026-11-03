//! Gunner sight presentation: the HUD widgets that draw the sight picture.
//! Crew status is the game's damage-model state: HP pools at stations.
//!
//! Reads mode, [`aim::CommittedAim`], dialed [`Ranging`] and rendered camera pose; writes only UI
//! nodes, never gun commands. Removing this layer cannot change [`super`]'s aim law.
//!
//! The interface is [`plugin`] (mount the whole layer) plus [`Toast`], the one widget the input half
//! needs to raise ("X unavailable" on a refused view switch).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::ui::UiSystems;

use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::aim::CommittedAim;
use crate::camera::GunnerCameraPlaced;
use crate::damage::{ControlledTank, TankKnockedOut};
use crate::damage_cam::pen_cam::game::KillCamGate;
use crate::firecontrol::{RangeTable, Ranging};
use crate::flow::{Flow, FlowScoped, InMatchSystems, MatchResourceExt};
use crate::overlay::{self, Overlay, Overlays};
use crate::spec::ViewKind;
use crate::state::GameplaySet;
use crate::tank::{Controlled, Hull, Rig, TankViews, ViewNode};
use crate::ui_font::{UiFonts, set_text};
use crate::view::PlayerView;

use super::{SightMode, sight_line, view_available};

mod look;
use look::GraticuleLook;

/// The on-screen intent cursor — the marker the gun chases. It moves immediately with the mouse
/// (position control) and drifts back to centre as the gun's lay catches up.
#[derive(Component)]
struct IntentReticle;

/// Full-screen black overlay shown when the active view's crewman is out of action, plus a center prompt
/// telling the player to switch to the other view. Hidden while that station is available.
#[derive(Component)]
struct ViewDeathOverlay;

/// Child text is separate from [`ViewDeathOverlay`]'s parent visibility.
#[derive(Component)]
struct ViewDeathText;

/// A brief on-screen message — used when a view switch is *refused* (the target view's crewman is
/// down), so the silent Lshift no-op gets a reason. Ticks down in `update_toast`.
///
/// [`Toast::show`] is the sole interface; private fields keep timing owned here.
#[derive(Resource, Default)]
pub(super) struct Toast {
    message: String,
    remaining: f32,
}

impl Toast {
    pub(super) fn show(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.remaining = crate::sight::look::get().toast_seconds.0;
    }
}

/// The toast's text node (upper-centre); shown while [`Toast::remaining`] > 0.
#[derive(Component)]
struct ToastText;

/// HUD dialed-range readout, hidden outside gunner view.
#[derive(Component)]
struct RangeReadout;

/// Horizontal zero mark on the gun's sight line; the scale slides behind it at the dialed graduation.
#[derive(Component)]
struct ReticleLine;

/// One moving range-scale graduation.
#[derive(Component)]
struct RangeScaleTick {
    range: f32,
    major: bool,
}

/// Mount the presentation layer: spawn the match's HUD nodes on entering a match (the view-death
/// black, an overlay, once at startup), then keep them in step with the sight.
pub(super) fn plugin(app: &mut App) {
    crate::flow::require(app, "sight");
    crate::sight::look::get();
    app.insert_resource(GraticuleLook::load())
        .init_match_resource::<Toast>()
        // [`OpticMaskMaterial`] owns the custom-shaded glass cutout.
        .add_plugins(UiMaterialPlugin::<OpticMaskMaterial>::default())
        .add_systems(Startup, spawn_view_death_overlay)
        .add_systems(
            OnEnter(Flow::InMatch),
            (
                spawn_intent_reticle,
                spawn_toast,
                spawn_range_readout,
                spawn_ranging_reticle,
                spawn_optic_mask,
            ),
        )
        .add_systems(
            Update,
            (
                // Declare before shared visibility reconciliation on net; SP writes visibility
                // directly and leaves Declare unconfigured. See `update_view_death_overlay`.
                update_view_death_overlay.in_set(overlay::OverlaySet::Declare),
                update_toast,
                update_range_readout,
            )
                .chain()
                // Show this frame's refusal after `super::toggle_sight`. Only the toggle is input;
                // overlay/toast/range keep updating with a free cursor, outside `PlayerInputSet`.
                .after(super::toggle_sight)
                .in_set(GameplaySet)
                .in_set(InMatchSystems),
        );
    mount_placement(app);
}

/// Mount the same projection schedule in the client and the headless layout fixture.
fn mount_placement(app: &mut App) {
    // Reproject after camera placement and transform propagation: graticule and camera read the
    // same VIEW gun GlobalTransform. Gunner intent commits before the fixed loop;
    // `interpolate_servos` presents servo angles in Update.
    app.add_systems(
        PostUpdate,
        (
            update_intent_reticle,
            update_ranging_reticle,
            update_optic_mask,
        )
            .in_set(GameplaySet)
            .after(TransformSystems::Propagate)
            .after(GunnerCameraPlaced)
            // Read this frame's target scale and write positions before this frame's layout.
            .after(UiSystems::Propagate)
            .before(UiSystems::Layout)
            .in_set(InMatchSystems),
    );
}

fn spawn_intent_reticle(mut commands: Commands) {
    let look = &crate::sight::look::get().intent;
    commands.spawn((
        FlowScoped(Flow::InMatch),
        IntentReticle,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(look.diameter_ui_px.0),
            height: Val::Px(look.diameter_ui_px.0),
            border_radius: BorderRadius::MAX,
            ..default()
        },
        BackgroundColor(look.colour()),
        Visibility::Hidden,
    ));
}

/// The full-screen black overlay + center prompt, shown when the active view's crewman is out of action.
/// The prompt offers Lshift only when the other view's station is available. An out-of-action
/// station supplies no sight picture in the game's damage model (design §7a).
fn spawn_view_death_overlay(mut commands: Commands, fonts: Res<UiFonts>) {
    let look = &crate::sight::look::get().view_prompt;
    commands
        .spawn((
            ViewDeathOverlay,
            // Game damage-model UI: the hook's lowest scrim z in SP/net keeps black below "YOU DIED"
            // when every crew station is out of action and Death is latched, regardless of spawn order.
            // On net, the marker delegates visibility to `overlay::apply_overlay_visibility`,
            // which suppresses black under higher scrim owners.
            overlay::OverlayNode(Overlay::ViewDead),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(crate::sight::look::get().view_surround()),
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            parent.spawn((
                ViewDeathText,
                Text::new(""),
                TextFont {
                    // SemiBold: a full-screen crew-death prompt.
                    font: fonts.hud.clone().into(),
                    font_size: FontSize::Px(look.font_ui_px.0),
                    ..default()
                },
                TextColor(look.colour()),
            ));
        });
}

/// Centred upper-third refusal banner, hidden until raised. Text and visibility share one entity/query.
fn spawn_toast(mut commands: Commands, fonts: Res<UiFonts>) {
    let look = crate::sight::look::get();
    commands
        .spawn((
            FlowScoped(Flow::InMatch),
            Node {
                width: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                top: Val::Percent(look.toast_top_percent.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|parent| {
            parent.spawn((
                ToastText,
                Text::new(""),
                TextFont {
                    // SemiBold: a centred refusal banner.
                    font: fonts.hud.clone().into(),
                    font_size: FontSize::Px(look.toast.font_ui_px.0),
                    ..default()
                },
                TextColor(look.toast.colour()),
                Visibility::Hidden,
            ));
        });
}

/// The dialed-range readout, parked bottom-left; populated/shown only in the optic.
fn spawn_range_readout(mut commands: Commands, fonts: Res<UiFonts>) {
    let look = crate::sight::look::get();
    commands.spawn((
        FlowScoped(Flow::InMatch),
        RangeReadout,
        Text::new(""),
        TextFont {
            // SemiBold: an all-caps gunnery readout ("RANGE ... m").
            font: fonts.hud.clone().into(),
            font_size: FontSize::Px(look.range.font_ui_px.0),
            ..default()
        },
        TextColor(look.range.colour()),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(look.range_inset_ui_px.0),
            left: Val::Px(look.range_inset_ui_px.0),
            ..default()
        },
        Visibility::Hidden,
    ));
}

/// Whether the dialed-range readout shows: in the gunner optic, and only over a controlled tank —
/// `SightMode` outlives a match, so the mode alone would print a range with nothing to range.
fn range_readout_visible(mode: SightMode, has_tank: bool) -> bool {
    has_tank && mode == SightMode::Gunner
}

/// Show the dialed range in the optic so the player can read and correct their estimate; hidden in
/// third-person (where scroll is the camera dolly, not ranging).
fn update_range_readout(
    mode: Res<SightMode>,
    ranging: Res<Ranging>,
    controlled: Query<(), With<Controlled>>,
    mut readout: Query<(&mut Text, &mut Visibility), With<RangeReadout>>,
) {
    let (mut text, mut visibility) = readout
        .single_mut()
        .expect("entering a match spawns exactly one range readout");
    if range_readout_visible(*mode, !controlled.is_empty()) {
        set_text(
            text.reborrow(),
            &format!("RANGE {} m", ranging.range as i32),
        );
        visibility.set_if_neq(Visibility::Visible);
    } else {
        visibility.set_if_neq(Visibility::Hidden);
    }
}

/// Zero mark and authored pool, numbered in hundreds of metres. [`update_ranging_reticle`]
/// absolutely positions them on the gun's sight line (off-centre at the authored blend) and shows
/// them only in the optic; [`place_mark`] supplies height.
fn spawn_ranging_reticle(mut commands: Commands, fonts: Res<UiFonts>, look: Res<GraticuleLook>) {
    let colour = look.colour();
    commands.spawn((
        FlowScoped(Flow::InMatch),
        ReticleLine,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(look.line_width_ui_px.0),
            ..default()
        },
        BackgroundColor(colour),
        Visibility::Hidden,
    ));

    for graduation in 1..=look.range_extent_m.0 / look.graduation_spacing_m.0 {
        let range = graduation * look.graduation_spacing_m.0;
        let major = range.is_multiple_of(look.major_interval_m.0);
        let range = f32::from(range);
        let width = look.tick_width(major);
        let mut tick = commands.spawn((
            FlowScoped(Flow::InMatch),
            RangeScaleTick { range, major },
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(width),
                ..default()
            },
            BackgroundColor(colour),
            Visibility::Hidden,
        ));
        if major {
            // Label rides the tick: an absolute child offsets from the tick's own top-left.
            tick.with_children(|parent| {
                parent.spawn((
                    Text::new(format!("{}", range / 100.0)),
                    TextFont {
                        // Regular weight keeps the graduation numbers legible at the authored size.
                        font: fonts.body.clone().into(),
                        font_size: FontSize::Px(look.label_font_ui_px.0),
                        ..default()
                    },
                    TextColor(colour),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(width + look.label_gap_ui_px.0),
                        top: Val::Px(look.label_top_ui_px.0),
                        ..default()
                    },
                ));
            });
        }
    }
}

/// Place one graticule mark as a hairline centred on `point`, in PHYSICAL pixels of the UI's target,
/// or hide it when the mark has nowhere to go. `target` is the mark node's own render-target info:
/// its scale factor (the target's, times `UiScale`; `bevy_ui::update::propagate_ui_target_cameras`)
/// is what layout multiplies this node's `Val::Px` by, so its reciprocal is one physical pixel.
///
/// The authored physical thickness stands half above and half below `point`. Layout
/// rounding (`LayoutConfig::use_rounding`, on by default) then puts both edges on the physical grid —
/// taffy's `round_layout` sets the height to `round(top + h) − round(top)` — so a one-pixel extent
/// covers exactly one physical row, and that row's centre is within half a pixel of `point`.
fn place_mark(
    node: &mut Node,
    visibility: &mut Visibility,
    point: Option<Vec2>,
    width: f32,
    target: &ComputedUiRenderTargetInfo,
    hairline_physical_px: f32,
) {
    match point {
        Some(point) => {
            let ui_px_per_physical = target.scale_factor().recip();
            node.left = Val::Px(point.x * ui_px_per_physical - width / 2.0);
            node.top = Val::Px((point.y - hairline_physical_px / 2.0) * ui_px_per_physical);
            node.height = Val::Px(hairline_physical_px * ui_px_per_physical);
            *visibility = Visibility::Visible;
        }
        None => *visibility = Visibility::Hidden,
    }
}

/// Each graduation sits `θ(dialed) − θ(range)` above the gun's sight line: dialed on [`ReticleLine`],
/// nearer above, farther below, following gun lay and range. Hidden outside the optic. Reads the
/// laid weapon's per-ammo range table (currently the main gun).
///
/// **The graticule is anchored to the gun, not to the camera.** Its zero is the sight line
/// (`super::sight_line`: the gun's lay minus the dialed superelevation), which is the line the shell
/// arcs back down onto and so the only line the dialed-range mark can name. That is NOT the green
/// bore dot, which is the barrel axis and by design rides the same superelevation ABOVE the sight
/// line. The anchor coincides with screen centre only at a blend of `0`; at the authored sight blend
/// the camera rides toward the intent and the scale correctly lags with the gun.
///
/// Read the render-smoothed VIEW gun (`interpolate_servos`, Update), as the optic camera and
/// `aim::update_bore_indicator` do: tick-rate sim poses would stutter against the view.
/// [`mount_placement`] orders reprojection after camera placement so marks and view share one pose.
#[expect(
    clippy::too_many_arguments,
    reason = "a system spanning sight mode, ranging, the controlled gun, range tables, the camera and two UI elements"
)]
fn update_ranging_reticle(
    mode: Res<SightMode>,
    ranging: Res<Ranging>,
    look: Res<GraticuleLook>,
    controlled: Query<&Rig, With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    gun: Query<&GlobalTransform>,
    tables: Query<&RangeTable>,
    camera: Query<(&Camera, &GlobalTransform), With<PlayerView>>,
    mut line: Query<
        (&mut Node, &mut Visibility, &ComputedUiRenderTargetInfo),
        (With<ReticleLine>, Without<RangeScaleTick>),
    >,
    mut ticks: Query<
        (
            &RangeScaleTick,
            &mut Node,
            &mut Visibility,
            &ComputedUiRenderTargetInfo,
        ),
        Without<ReticleLine>,
    >,
) {
    let (camera, cam_transform) = camera
        .single()
        .expect("the client constructs exactly one player camera before entering a match");
    // Outside the optic, before rig/view-gun bind, or without a viewport, all marks hide together.
    // The muzzle must receive its range table at spawn; a missing table is a bug.
    let mark = (*mode == SightMode::Gunner)
        .then(|| {
            let rig = crate::tank::controlled_one(controlled.single())?;
            let table = tables
                .get(rig.muzzle)
                .expect("the primary muzzle receives its range table at spawn");
            let gun = gun
                .get(ViewNode::resolve(view_nodes.get(rig.gun).ok(), rig.gun))
                .ok()?;
            let viewport = camera.logical_viewport_rect()?;
            // Convert camera-logical projection to physical hairlines. UI and view share the
            // window/capture target, so physical pixels agree.
            let to_physical = camera
                .target_scaling_factor()
                .expect("a camera with a logical viewport has the target scale it divided by");
            let dialed = table.superelevation(ranging.range);
            let sight = sight_line(gun.rotation(), dialed);
            // Camera and sight share this origin: bearing alone decides placement.
            let origin = gun.translation();
            // Marks stack about the CAMERA's right axis, so the scale reads screen-vertical whatever
            // the hull's roll.
            let right = cam_transform.rotation() * Vec3::X;
            Some(move |range: f32| {
                let dir =
                    Quat::from_axis_angle(right, dialed - table.superelevation(range)) * sight;
                camera
                    .world_to_viewport(cam_transform, origin + dir)
                    .ok()
                    // A mark past the viewport's edge is HIDDEN, never clamped onto it: the scale
                    // extends beyond the optic's reach, and `world_to_viewport`
                    // answers off-screen coordinates for anything in front of the camera (it only
                    // fails behind the near plane / past the far plane).
                    .filter(|screen| viewport.contains(*screen))
                    .map(|screen| screen * to_physical)
            })
        })
        .flatten();

    // `θ(dialed) − θ(dialed) = 0`: zero mark and dialed graduation cannot separate.
    let (mut node, mut visibility, target) = line
        .single_mut()
        .expect("entering a match spawns exactly one ranging zero mark");
    let point = mark.as_ref().and_then(|mark| mark(ranging.range));
    place_mark(
        &mut node,
        &mut visibility,
        point,
        look.line_width_ui_px.0,
        target,
        look.hairline_physical_px.0,
    );
    for (tick, mut node, mut visibility, target) in &mut ticks {
        let point = mark.as_ref().and_then(|mark| mark(tick.range));
        place_mark(
            &mut node,
            &mut visibility,
            point,
            look.tick_width(tick.major),
            target,
            look.hairline_physical_px.0,
        );
    }
}

/// The drawn rim in shader height units; cursor reach consumes the same authored span.
fn mask_rim(viewport: Vec2) -> (f32, f32) {
    let look = crate::sight::look::get();
    let radius = 0.5 * look.optic_span_fraction.0 * viewport.max_element() / viewport.y;
    (radius, look.optic_feather_fraction.0 * radius)
}

/// Resolution: finite seed for the hidden material; placement overwrites it before visibility.
const OPTIC_GLASS_UNPLACED: f32 = 16.0;

/// The uniform block `assets/shaders/optic_mask.wgsl` reads (lane map in the shader).
#[derive(ShaderType, Clone, Copy, Debug)]
struct OpticMaskParams {
    /// xy: the glass centre in node UV. z: its radius, w: the edge feather's half-width — both as
    /// fractions of the node's HEIGHT.
    glass: Vec4,
    /// What lies outside the glass, linear RGBA.
    surround: Vec4,
}

/// Shader cutout stays circular at any aspect/centre. Rectangles cannot: every blend but `0`
/// moves the glass off the node's centre.
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
struct OpticMaskMaterial {
    #[uniform(0)]
    params: OpticMaskParams,
}

impl UiMaterial for OpticMaskMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/optic_mask.wgsl".into()
    }
}

/// Identifies material and visibility for [`update_optic_mask`].
#[derive(Component)]
struct OpticMask;

/// Hidden match-entry surround at `overlay::Overlay::SIGHT_FURNITURE_Z`: below graticule, intent
/// cursor and overlays that cover the sight picture.
fn spawn_optic_mask(mut commands: Commands, mut materials: ResMut<Assets<OpticMaskMaterial>>) {
    commands.spawn((
        FlowScoped(Flow::InMatch),
        OpticMask,
        MaterialNode(materials.add(OpticMaskMaterial {
            // Placed by `update_optic_mask` before it is ever shown; a fully open glass is the
            // harmless pre-placement state.
            params: OpticMaskParams {
                glass: Vec4::new(
                    0.5,
                    0.5,
                    OPTIC_GLASS_UNPLACED,
                    crate::sight::look::get().optic_feather_fraction.0 * OPTIC_GLASS_UNPLACED,
                ),
                surround: Vec4::from_array(crate::sight::look::get().optic_surround_linear_rgba.0),
            },
        })),
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        GlobalZIndex(Overlay::SIGHT_FURNITURE_Z),
        Visibility::Hidden,
    ));
}

/// The drawn glass, in the units the mask shader wants: a centre in node UV, and a radius and edge
/// feather as fractions of the node's HEIGHT.
struct OpticGlass {
    centre: Vec2,
    radius: f32,
    feather: f32,
}

/// Project the gun's sight line: camera/viewport-centred only at blend `0`. At the authored blend
/// the glass slides as the gun lags. Pass off-rect centres unclamped: their visible arcs are correct;
/// clamping would drag the glass off the sight line.
///
/// Projection errors hide the mask. `world_to_viewport` permits off-rect X/Y but also rejects clip
/// depth, missing viewport and invalid projection data, not just points behind the camera
/// (bevy_camera-0.19.1/src/camera.rs:539-559, `world_to_viewport_core`).
///
/// [`mask_rim`] uses only the viewport; cursor reach projects the same fraction of the same axis,
/// so its circle matches the glass at every aspect. The optic FOV supplies the bound's other term.
fn optic_glass(
    camera: &Camera,
    cam_transform: &GlobalTransform,
    sight: Vec3,
) -> Option<OpticGlass> {
    let viewport = camera.logical_viewport_size()?;
    // Height is the unit: a zero-height/minimised target has no radius, rather than a NaN.
    if viewport.y <= 0.0 {
        return None;
    }
    let eye = cam_transform.translation();
    // Projection is scale-invariant along a ray, but f32 spacing quantizes `eye + dir`. MEASURED:
    // a one-metre step a kilometre from origin shifts the centre a fifth of a pixel. Sampling a
    // whole eye-distance out holds angular error to one f32 epsilon anywhere on the map.
    let reach = eye.length().max(1.0);
    let centre = camera
        .world_to_viewport(cam_transform, eye + sight * reach)
        .ok()?;
    let (radius, feather) = mask_rim(viewport);
    Some(OpticGlass {
        centre: centre / viewport,
        radius,
        feather,
    })
}

/// Put the glass on the gun's sight line and show the surround; hidden outside the optic, and
/// whenever the sight picture has no place on screen at all.
///
/// Uses the graticule/camera's VIEW gun and [`sight_line`]; [`mount_placement`] orders it after
/// camera placement so glass, zero mark and view cannot drift.
#[expect(
    clippy::too_many_arguments,
    reason = "a system spanning sight mode, ranging, the controlled gun, range tables, the camera and the mask material"
)]
fn update_optic_mask(
    mode: Res<SightMode>,
    ranging: Res<Ranging>,
    controlled: Query<&Rig, With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    gun: Query<&GlobalTransform>,
    tables: Query<&RangeTable>,
    camera: Query<(&Camera, &GlobalTransform), With<PlayerView>>,
    mut materials: ResMut<Assets<OpticMaskMaterial>>,
    mut mask: Query<(&MaterialNode<OpticMaskMaterial>, &mut Visibility), With<OpticMask>>,
) {
    let (node, mut visibility) = mask
        .single_mut()
        .expect("entering a match spawns exactly one optic mask");
    let mut material = materials
        .get_mut(&node.0)
        .expect("the optic mask material is created synchronously with its match-entry widget");
    let (camera, cam_transform) = camera
        .single()
        .expect("the client constructs exactly one player camera before entering a match");
    let glass = (*mode == SightMode::Gunner)
        .then(|| {
            let rig = crate::tank::controlled_one(controlled.single())?;
            let gun = gun
                .get(ViewNode::resolve(view_nodes.get(rig.gun).ok(), rig.gun))
                .ok()?;
            let theta = tables
                .get(rig.muzzle)
                .expect("the primary muzzle receives its range table at spawn")
                .superelevation(ranging.range);
            optic_glass(camera, cam_transform, sight_line(gun.rotation(), theta))
        })
        .flatten();

    let Some(glass) = glass else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    material.params.glass = glass.centre.extend(glass.radius).extend(glass.feather);
    visibility.set_if_neq(Visibility::Visible);
}

/// Reproject the committed world point at any range, resumed from third person or resolved in the optic.
/// A bearing-only shortcut puts near-floor aim too high by mount parallax. The cursor returns to
/// centre as gun/camera catch up; hidden outside gunner view.
///
/// Reads [`aim::CommittedAim`] republished by `super::drive_gunner_aim` in BeforeFixedMainLoop
/// and camera pose from the VIEW gun GlobalTransform (`interpolate_servos`, Update). Both inputs
/// are from this frame, without aliasing.
fn update_intent_reticle(
    mode: Res<SightMode>,
    committed: Res<CommittedAim>,
    camera: Query<(&Camera, &GlobalTransform), With<PlayerView>>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    hull: Query<&GlobalTransform, With<Hull>>,
    mut reticle: Query<
        (&mut Node, &mut Visibility, &ComputedUiRenderTargetInfo),
        With<IntentReticle>,
    >,
) {
    let (mut node, mut visibility, target) = reticle
        .single_mut()
        .expect("entering a match spawns exactly one intent cursor");
    if *mode != SightMode::Gunner {
        *visibility = Visibility::Hidden;
        return;
    }
    let Some((tank, rig)) = crate::tank::controlled_one(controlled.single()) else {
        *visibility = Visibility::Hidden;
        return;
    };
    let hull = hull
        .get(rig.hull)
        .expect("the complete rig's hull has its global pose");
    // Before the first aim commit, there is no intent point to draw.
    let Some(local) = committed.get(tank) else {
        *visibility = Visibility::Hidden;
        return;
    };
    let (camera, cam_transform) = camera
        .single()
        .expect("the client constructs exactly one player camera before entering a match");

    let point = hull.affine().transform_point3(local);

    match camera.world_to_viewport(cam_transform, point) {
        Ok(screen) => {
            // Camera logical pixels -> physical target pixels -> this node's UI pixels.
            let to_ui = camera
                .target_scaling_factor()
                .expect("a successful viewport projection has the camera's target scale")
                / target.scale_factor();
            let screen = screen * to_ui;
            node.left = Val::Px(screen.x - crate::sight::look::get().intent.diameter_ui_px.0 / 2.0);
            node.top = Val::Px(screen.y - crate::sight::look::get().intent.diameter_ui_px.0 / 2.0);
            *visibility = Visibility::Visible;
        }
        Err(_) => *visibility = Visibility::Hidden,
    }
}

/// Whether the kill cam owns the controlled tank's view: the kill cam is mounted ([`KillCamGate`])
/// and the controlled tank is knocked out.
#[derive(SystemParam)]
struct KillCamView<'w, 's> {
    gate: Option<Res<'w, KillCamGate>>,
    knocked_out: Query<'w, 's, (), (With<Controlled>, With<TankKnockedOut>)>,
}

impl KillCamView<'_, '_> {
    fn owns(&self) -> bool {
        self.gate.is_some() && !self.knocked_out.is_empty()
    }
}

/// Show/hide the black overlay + prompt when the active view's crewman is out of action. The prompt tells
/// the player to press Lshift to switch to the other view if its crewman is available; if both
/// view stations are out of action, the prompt says so.
///
/// Net declares `Overlay::ViewDead` and refreshes text in [`overlay::OverlaySet::Declare`];
/// `overlay::apply_overlay_visibility` then reads the complete set, suppressing this black under
/// death, menu or connect scrims. SP has no `Overlays`; in the game's damage model an out-of-action
/// station shows black + prompt directly.
///
/// Where the kill cam is mounted ([`KillCamGate`]), a knocked-out tank's view is the kill cam's and
/// then its death screen's: no black.
fn update_view_death_overlay(
    mode: Res<SightMode>,
    controlled: ControlledTank,
    kill_cam: KillCamView,
    views: Query<&TankViews, With<Controlled>>,
    overlays: Option<ResMut<Overlays>>,
    mut overlay_vis: Query<&mut Visibility, With<ViewDeathOverlay>>,
    mut label: Query<&mut Text, With<ViewDeathText>>,
) {
    let has_controlled = controlled.entity().is_some();

    let (active_view, other_view, other_label) = match *mode {
        SightMode::ThirdPerson => (ViewKind::Commander, ViewKind::Gunner, "gunner optic"),
        SightMode::Gunner => (ViewKind::Gunner, ViewKind::Commander, "third-person"),
    };

    // The active view's crewman is down — the standalone condition for wanting this overlay. Gated on a
    // controlled tank existing (there is no station without a tank).
    let crewman_down =
        has_controlled && !kill_cam.owns() && !view_available(&controlled, &views, active_view);

    // Net only declares; a visibility write here would double-write from an unreconciled set.
    // Declare before node lookup: presence follows the game's damage-model state, not view hierarchy.
    // Skipping this absolute declaration would latch the previous answer instead of clearing it.
    let single_player = match overlays {
        Some(mut overlays) => {
            overlays.declare(Overlay::ViewDead, crewman_down);
            false
        }
        None => true,
    };

    // The overlay's `Visibility` lives on the full-screen node; its prompt `Text` on the child.
    let mut vis = overlay_vis
        .single_mut()
        .expect("startup spawns exactly one view-death overlay");
    let mut text = label
        .single_mut()
        .expect("the view-death overlay spawns exactly one prompt");

    // Refresh before showing in either mode; stale hidden text is harmless.
    if crewman_down {
        let other_available = view_available(&controlled, &views, other_view);
        let prompt = if other_available {
            format!("Crewman out of action — [Lshift] for {other_label}")
        } else {
            "All view crew out of action".to_string()
        };
        set_text(text.reborrow(), &prompt);
    }

    // Single-player: no authority — draw whenever the active crewman is down.
    if single_player {
        vis.set_if_neq(if crewman_down {
            Visibility::Visible
        } else {
            Visibility::Hidden
        });
    }
}

/// Tick the refusal toast: show its message while it has time left, then hide it. Set by
/// `super::toggle_sight` when a view switch is refused (the target view's crewman is down).
fn update_toast(
    time: Res<Time>,
    mut toast: ResMut<Toast>,
    mut label: Query<(&mut Text, &mut Visibility), With<ToastText>>,
) {
    let (mut text, mut visibility) = label
        .single_mut()
        .expect("entering a match spawns exactly one refusal toast");
    if toast.remaining > 0.0 {
        toast.remaining -= time.delta_secs();
        set_text(text.reborrow(), &toast.message);
        visibility.set_if_neq(Visibility::Visible);
    } else if *visibility != Visibility::Hidden {
        *visibility = Visibility::Hidden;
    }
}

#[cfg(test)]
mod tests {
    /// Sight appearance must reach the shipped widget spawners; retunes cannot leave a code copy.
    #[test]
    fn sight_widget_appearance_comes_from_data() {
        fn label<M: Component>(world: &mut World, look: &crate::sight::look::LabelLook) {
            let (font, colour) = world
                .query_filtered::<(&TextFont, &TextColor), With<M>>()
                .single(world)
                .unwrap();
            assert_eq!(font.font_size, FontSize::Px(look.font_ui_px.0));
            assert_eq!(colour.0.to_linear(), look.colour().to_linear());
        }

        let mut world = World::new();
        world.insert_resource(UiFonts {
            hud: Handle::default(),
            body: Handle::default(),
        });
        world.init_resource::<Assets<OpticMaskMaterial>>();
        world.run_system_once(spawn_intent_reticle).unwrap();
        world.run_system_once(spawn_view_death_overlay).unwrap();
        world.run_system_once(spawn_toast).unwrap();
        world.run_system_once(spawn_range_readout).unwrap();
        world.run_system_once(spawn_optic_mask).unwrap();
        let look = crate::sight::look::get();
        let (node, colour) = world
            .query_filtered::<(&Node, &BackgroundColor), With<IntentReticle>>()
            .single(&world)
            .unwrap();
        assert_eq!(node.width, Val::Px(look.intent.diameter_ui_px.0));
        assert_eq!(node.height, Val::Px(look.intent.diameter_ui_px.0));
        assert_eq!(colour.0.to_linear(), look.intent.colour().to_linear());
        let backing = world
            .query_filtered::<&BackgroundColor, With<ViewDeathOverlay>>()
            .single(&world)
            .unwrap();
        assert_eq!(backing.0.to_linear(), look.view_surround().to_linear());
        label::<ViewDeathText>(&mut world, &look.view_prompt);
        label::<ToastText>(&mut world, &look.toast);
        let toast_parent = world
            .query_filtered::<&ChildOf, With<ToastText>>()
            .single(&world)
            .unwrap()
            .parent();
        assert_eq!(
            world.get::<Node>(toast_parent).unwrap().top,
            Val::Percent(look.toast_top_percent.0)
        );
        label::<RangeReadout>(&mut world, &look.range);
        let readout = world
            .query_filtered::<&Node, With<RangeReadout>>()
            .single(&world)
            .unwrap();
        assert_eq!(readout.left, Val::Px(look.range_inset_ui_px.0));
        assert_eq!(readout.bottom, Val::Px(look.range_inset_ui_px.0));
        let material = world
            .query_filtered::<&MaterialNode<OpticMaskMaterial>, With<OpticMask>>()
            .single(&world)
            .unwrap();
        assert_eq!(
            world
                .resource::<Assets<OpticMaskMaterial>>()
                .get(&material.0)
                .unwrap()
                .params
                .surround,
            Vec4::from_array(look.optic_surround_linear_rgba.0)
        );
        let mut toast = Toast::default();
        toast.show("fixture");
        assert_eq!(toast.remaining, look.toast_seconds.0);
    }

    use bevy::camera::{ComputedCameraValues, RenderTargetInfo};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::Affine3A;

    use super::*;
    use crate::sight::{BLEND_SAMPLES, hull_local_dir, optic_margin, yaw_pitch_of};
    use crate::spec::Optics;

    fn assert_invariant_failure(failure: std::thread::Result<()>, invariant: &str) {
        let panic = failure.expect_err("a broken widget invariant must stop the update");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("the invariant panic has a message");
        assert!(message.contains(invariant), "{message}");
    }

    /// Match-entry construction supplies exactly one cursor and zero mark. A broken pool must
    /// stop with its cause instead of leaving the player looking at frozen sight furniture.
    #[test]
    fn required_reticle_cardinalities_fail_visibly() {
        for count in [0, 2] {
            let mut picture = SightPicture::new(Quat::IDENTITY);
            picture.world.insert_resource(CommittedAim::default());
            for _ in 0..count {
                picture
                    .world
                    .spawn((IntentReticle, Node::default(), Visibility::Hidden));
            }
            let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                picture
                    .world
                    .run_system_once(update_intent_reticle)
                    .unwrap();
            }));
            assert_invariant_failure(failure, "exactly one intent cursor");

            let mut picture = SightPicture::new(Quat::IDENTITY);
            picture.world.despawn(picture.line);
            for _ in 0..count {
                picture
                    .world
                    .spawn((ReticleLine, Node::default(), Visibility::Hidden));
            }
            let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| picture.run()));
            assert_invariant_failure(failure, "exactly one ranging zero mark");
        }
    }

    /// The mask's strong handle names a material created synchronously at match entry. Losing
    /// that material must stop the update before a stale or unbound mask can be shown.
    #[test]
    fn optic_mask_requires_its_synchronous_material() {
        let mut picture = SightPicture::new(Quat::IDENTITY);
        picture.bolt_to_sight_line();
        picture.world.init_resource::<Assets<OpticMaskMaterial>>();
        picture.world.run_system_once(spawn_optic_mask).unwrap();
        // Fixture fault: remove the synchronously inserted asset while its widget survives.
        picture
            .world
            .insert_resource(Assets::<OpticMaskMaterial>::default());
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            picture.world.run_system_once(update_optic_mask).unwrap();
        }));
        assert_invariant_failure(failure, "the optic mask material is created synchronously");
    }

    /// The fixture's viewport, in px.
    const VIEWPORT: UVec2 = UVec2::new(1280, 720);
    /// Fixture instrument: a middling gunnery sight.
    const OPTICS: Optics = Optics::Magnified {
        magnification: 2.5,
        field_deg: 25.0,
    };
    /// Fixture FOV, rad, through the client conversion. DERIVED on-axis at 720 px / 25°:
    /// 1 mrad is ~1.62 px; DIALED's 6.4 mrad bore/sight gap is ~10.4 px.
    const FOV: f32 = OPTICS.vertical_fov();
    /// Where the gun node stands: far from the world origin, so a placement that composes the wrong
    /// frame moves instead of cancelling.
    const GUN: Vec3 = Vec3::new(137.0, 4.6, -512.0);
    /// The dialed range (m) — on a graduation, so the zero mark and one tick must coincide.
    const DIALED: f32 = 800.0;
    /// Angular tolerance, rad. DERIVED on-axis: EPS × 720 / (2 tan(FOV/2)) ≈ 0.16 px.
    const EPS: f32 = 1e-4;

    fn fixture_look() -> GraticuleLook {
        use crate::provenance::Sourced;
        let mut look = GraticuleLook::load();
        // Fixture: physical-grid tests exercise one device row independently of shipped thickness.
        look.hairline_physical_px = Sourced(1.0);
        // Fixture: the geometric laws sample a complete scale with DIALED on a graduation.
        look.graduation_spacing_m = Sourced(200);
        look.major_interval_m = Sourced(400);
        look.range_extent_m = Sourced(4000);
        look
    }

    /// A superelevation that RISES with range across the whole graticule, so every graduation
    /// interpolates rather than saturating on an end row.
    fn table() -> RangeTable {
        RangeTable::test_rows(&[(0.0, 0.0), (1000.0, 0.008), (4000.0, 0.045)])
    }

    /// `acos(dot)` loses f32 precision near 1; these directions should differ by zero.
    fn angle_between(a: Vec3, b: Vec3) -> f32 {
        a.cross(b).length().atan2(a.dot(b))
    }

    fn px(val: Val) -> f32 {
        match val {
            Val::Px(px) => px,
            other => panic!("a graticule mark is placed in px, got {other:?}"),
        }
    }

    /// Production graduation spawn/placement with fixture spacing, gun lay and camera blend pose
    /// (on the sight line at `k = 0`, off it above); no reimplemented placement law.
    struct SightPicture {
        world: World,
        camera: Entity,
        gun: Entity,
        line: Entity,
        /// Graduation entities paired with range.
        ticks: Vec<(Entity, f32)>,
    }

    impl SightPicture {
        fn new(lay: Quat) -> Self {
            Self::with_look(lay, fixture_look())
        }

        fn with_look(lay: Quat, look: GraticuleLook) -> Self {
            let mut world = World::new();
            world.insert_resource(look);
            world.insert_resource(SightMode::Gunner);
            world.insert_resource(Ranging { range: DIALED });
            world.insert_resource(UiFonts {
                hud: Handle::default(),
                body: Handle::default(),
            });

            let camera = world
                .spawn((
                    Camera {
                        computed: ComputedCameraValues {
                            clip_from_view: Mat4::perspective_infinite_reverse_rh(
                                FOV,
                                VIEWPORT.x as f32 / VIEWPORT.y as f32,
                                0.1,
                            ),
                            target_info: Some(RenderTargetInfo {
                                physical_size: VIEWPORT,
                                scale_factor: 1.0,
                            }),
                            ..default()
                        },
                        ..default()
                    },
                    // Reticle systems resolve cameras through `view::PlayerView`.
                    PlayerView,
                    GlobalTransform::default(),
                ))
                .id();
            let gun = world.spawn(GlobalTransform::default()).id();
            let muzzle = world.spawn(table()).id();
            let hull = world.spawn_empty().id();
            world.spawn((
                Controlled,
                Rig {
                    hull,
                    turret: hull,
                    gun,
                    muzzle,
                },
            ));
            world
                .run_system_once(spawn_ranging_reticle)
                .expect("the shipped spawner runs");
            let line = world
                .query_filtered::<Entity, With<ReticleLine>>()
                .single(&world)
                .expect("the zero mark is spawned");
            let ticks = world
                .query::<(Entity, &RangeScaleTick)>()
                .iter(&world)
                .map(|(entity, tick)| (entity, tick.range))
                .collect();

            let mut picture = Self {
                world,
                camera,
                gun,
                line,
                ticks,
            };
            picture.lay(lay);
            picture
        }

        /// Lay the gun: the node's world attitude, whose −Z is the bore.
        fn lay(&mut self, rotation: Quat) {
            let pose = Transform {
                translation: GUN,
                rotation,
                ..default()
            };
            self.world
                .entity_mut(self.gun)
                .insert(GlobalTransform::from(pose));
        }

        /// Pose the camera at the gun mount (where the optic camera parks) looking along `look` —
        /// the blend decides only what that look is.
        fn look_along(&mut self, look: Quat) {
            self.world
                .entity_mut(self.camera)
                .insert(GlobalTransform::from(Transform {
                    translation: GUN,
                    rotation: look,
                    ..default()
                }));
        }

        /// The `k = 0` camera, built exactly as `camera::gunner_camera` builds it there: parked at
        /// the gun node, looking along the sight line, up = the node's own +Y.
        fn bolt_to_sight_line(&mut self) {
            let rotation = self.gun_rotation();
            let pose = Transform::from_translation(GUN)
                .looking_to(sight_line(rotation, self.dialed_lob()), rotation * Vec3::Y);
            self.world
                .entity_mut(self.camera)
                .insert(GlobalTransform::from(pose));
        }

        fn run(&mut self) {
            self.world
                .run_system_once(update_ranging_reticle)
                .expect("the shipped placement runs");
        }

        fn gun_rotation(&self) -> Quat {
            self.world
                .get::<GlobalTransform>(self.gun)
                .expect("the gun node carries a pose")
                .rotation()
        }

        fn camera_rotation(&self) -> Quat {
            self.world
                .get::<GlobalTransform>(self.camera)
                .expect("the camera carries a pose")
                .rotation()
        }

        /// Dialed superelevation through the shipped table lookup.
        fn dialed_lob(&self) -> f32 {
            table().superelevation(DIALED)
        }

        /// The gun's sight line and its bore: the two lines the graticule must tell apart.
        fn sight(&self) -> Vec3 {
            sight_line(self.gun_rotation(), self.dialed_lob())
        }

        fn bore(&self) -> Vec3 {
            self.gun_rotation() * Vec3::NEG_Z
        }

        /// Zero-mark screen centre; `None` means hidden.
        fn zero_mark(&self) -> Option<Vec2> {
            self.placed(self.line)
        }

        /// `(range, screen mark)` pairs.
        fn graduations(&self) -> Vec<(f32, Option<Vec2>)> {
            self.ticks
                .iter()
                .map(|&(entity, range)| (range, self.placed(entity)))
                .collect()
        }

        /// The mark's centre in the camera's logical pixels: its node's UI px, through the physical
        /// pixels [`place_mark`] draws in.
        fn placed(&self, entity: Entity) -> Option<Vec2> {
            let visibility = self.world.get::<Visibility>(entity)?;
            if *visibility == Visibility::Hidden {
                return None;
            }
            let node = self.world.get::<Node>(entity)?;
            let ui_scale = self
                .world
                .get::<ComputedUiRenderTargetInfo>(entity)
                .expect("a node requires its render-target info")
                .scale_factor();
            let camera_scale = self
                .world
                .get::<Camera>(self.camera)
                .and_then(Camera::target_scaling_factor)
                .expect("the fixture camera has a target");
            let centre = Vec2::new(
                px(node.left) + px(node.width) / 2.0,
                px(node.top) + px(node.height) / 2.0,
            );
            Some(centre * ui_scale / camera_scale)
        }

        /// Read back through the camera so tests assert sight angles rather than pixel offsets.
        fn bearing(&self, screen: Vec2) -> Vec3 {
            let camera = self.world.get::<Camera>(self.camera).expect("a camera");
            let transform = self
                .world
                .get::<GlobalTransform>(self.camera)
                .expect("a camera pose");
            camera
                .viewport_to_world(transform, screen)
                .expect("a mark inside the viewport unprojects")
                .direction
                .into()
        }

        /// Elevation in the graticule plane, about camera right. That rotation rigidly turns
        /// camera-local `(y, −z)`, reading placement angle exactly at any screen position.
        fn elevation(&self, dir: Vec3) -> f32 {
            let local = self.camera_rotation().inverse() * dir;
            local.y.atan2(-local.z)
        }
    }

    /// With camera fixed on intent (`k > 0`), marks must travel by the gun's lay change. A
    /// camera-forward anchor would not move.
    #[test]
    fn the_graticule_travels_with_the_gun() {
        /// How far the gun elevates between the two frames (rad) — inside the optic's half-FOV, so
        /// the mark is on screen either side.
        const LAY_STEP: f32 = 0.02;

        let mut picture = SightPicture::new(Quat::IDENTITY);
        picture.look_along(Quat::IDENTITY);
        picture.run();
        let before = picture.zero_mark().expect("the zero mark starts on screen");

        picture.lay(Quat::from_rotation_x(LAY_STEP));
        picture.run();
        let after = picture.zero_mark().expect("the zero mark stays on screen");

        let travelled =
            picture.elevation(picture.bearing(after)) - picture.elevation(picture.bearing(before));
        assert!(
            (travelled - LAY_STEP).abs() < EPS,
            "the graticule must travel with the gun: the gun elevated {LAY_STEP} rad under a still \
             camera and the zero mark moved {travelled} rad — zero means it is still anchored to \
             the camera",
        );
    }

    /// Zero/dialed marks must name the sight line; the bore dot stays θ higher. The
    /// [`update_ranging_reticle`] doc owns why. Measure with camera off that line (`k > 0`) so
    /// camera anchoring cannot pass by coincidence.
    #[test]
    fn the_zero_mark_is_the_sight_line_a_superelevation_below_the_bore() {
        let mut picture = SightPicture::new(Quat::from_rotation_y(0.03));
        // The camera leads the gun, as a blend toward the intent cursor leaves it.
        picture.look_along(Quat::from_rotation_y(0.05) * Quat::from_rotation_x(0.01));
        picture.run();

        let zero = picture.bearing(picture.zero_mark().expect("the zero mark is on screen"));
        let sight_error = angle_between(zero, picture.sight());
        assert!(
            sight_error < EPS,
            "the zero mark must sit on the gun's sight line, off by {sight_error} rad",
        );

        let theta = picture.dialed_lob();
        let separation = picture.elevation(picture.bore()) - picture.elevation(zero);
        assert!(
            theta > 0.0 && (separation - theta).abs() < EPS,
            "the bore must stand exactly the dialed superelevation ({theta} rad) above the zero \
             mark — got {separation} rad; equal to zero would mean the bar was pinned to the bore",
        );

        // The dialed range has its own graduation, and it is the same place.
        let dialed = picture
            .graduations()
            .into_iter()
            .find_map(|(range, mark)| (range == DIALED).then_some(mark))
            .expect("the dialed range is a graduation")
            .expect("and it is on screen");
        let split = angle_between(picture.bearing(dialed), zero);
        assert!(
            split < EPS,
            "the dialed graduation and the zero mark are one place, split by {split} rad",
        );
    }

    /// Assert [`update_ranging_reticle`]'s table-derived spacing and nearer-above/farther-below rule.
    #[test]
    fn every_graduation_stands_at_its_superelevation_difference() {
        let mut picture =
            SightPicture::new(Quat::from_rotation_y(0.02) * Quat::from_rotation_x(0.01));
        picture.bolt_to_sight_line();
        picture.run();

        let table = table();
        let zero = picture.elevation(picture.sight());
        let mut checked = 0;
        for (range, mark) in picture.graduations() {
            let Some(mark) = mark else {
                continue; // a graduation past the optic's reach is hidden, not misplaced
            };
            let above = picture.elevation(picture.bearing(mark)) - zero;
            let want = table.superelevation(DIALED) - table.superelevation(range);
            assert!(
                (above - want).abs() < EPS,
                "the {range} m graduation must stand θ(dialed) − θ(range) = {want} rad above the \
                 sight line, got {above} rad",
            );
            if range != DIALED {
                assert_eq!(
                    range < DIALED,
                    above > 0.0,
                    "a nearer range must read ABOVE the dialed mark and a farther one below it; \
                     the {range} m graduation stands {above} rad from it",
                );
            }
            checked += 1;
        }
        assert!(
            checked > 4,
            "the law must be measured on the scale, not on a stray mark — only {checked} \
             graduations were on screen",
        );
    }

    /// **The graticule hides rather than smears.** A mark that cannot be drawn where it belongs is
    /// not drawn at all — off the side of the viewport (still in front of the camera, which
    /// `world_to_viewport` answers with off-screen coordinates rather than an error) and behind it —
    /// and outside the optic nothing is drawn at all. The blend keeps the sight line itself well
    /// inside the glass, so this is the far graduations' law; it is measured on the whole graticule
    /// by pushing the camera off the sight line further than any blend can.
    #[test]
    fn the_graticule_hides_whole_rather_than_smearing() {
        let mut picture = SightPicture::new(Quat::IDENTITY);

        for (case, look) in [
            ("off the side of the viewport", Quat::from_rotation_y(0.5)),
            (
                "behind the camera",
                Quat::from_rotation_y(std::f32::consts::PI),
            ),
        ] {
            picture.look_along(look);
            picture.run();
            assert!(
                picture.zero_mark().is_none(),
                "the zero mark must hide with the sight line {case}",
            );
            assert!(
                picture.graduations().iter().all(|(_, mark)| mark.is_none()),
                "no graduation may be drawn at the edge with the sight line {case}",
            );
        }

        // Outside the optic the whole graticule is down, wherever the gun is pointing.
        picture.look_along(Quat::IDENTITY);
        picture.run();
        assert!(picture.zero_mark().is_some(), "the optic draws it");
        *picture.world.resource_mut::<SightMode>() = SightMode::ThirdPerson;
        picture.run();
        assert!(
            picture.zero_mark().is_none()
                && picture.graduations().iter().all(|(_, mark)| mark.is_none()),
            "third person has no ranging reticle",
        );
    }

    /// Sight authors own every graticule look value. A different inline fixture must reach
    /// the production spawner and placer; a constant left in either path breaks this invariant.
    #[test]
    fn authored_graticule_look_reaches_marks_and_labels() {
        // Fixture: deliberately distinct sizes, offsets and colour expose any kept code taste.
        let look = ron::from_str::<GraticuleLook>(
            r#"(
            graduation_spacing_m: (300, Fixture("inline graduation spacing")),
            major_interval_m: (600, Fixture("inline numbered interval")),
            range_extent_m: (1800, Fixture("inline scale extent")),
            hairline_physical_px: (3.0, Fixture("inline thickness")),
            line_width_ui_px: (57.0, Fixture("inline zero width")),
            major_tick_width_ui_px: (31.0, Fixture("inline major width")),
            minor_tick_width_ui_px: (17.0, Fixture("inline minor width")),
            label_gap_ui_px: (9.0, Fixture("inline label clearance")),
            label_top_ui_px: (-11.0, Fixture("inline label offset")),
            label_font_ui_px: (13.0, Fixture("inline label font")),
            colour_srgba: ((0.2, 0.3, 0.4, 0.9), Fixture("inline colour")),
        )"#,
        )
        .unwrap();
        let mut picture = SightPicture::with_look(Quat::IDENTITY, look);
        picture.run();
        let mut ranges = picture
            .ticks
            .iter()
            .map(|(_, range)| *range)
            .collect::<Vec<_>>();
        ranges.sort_by(f32::total_cmp);
        assert_eq!(ranges, vec![300.0, 600.0, 900.0, 1200.0, 1500.0, 1800.0]);
        let world = &mut picture.world;
        let line = world.get::<Node>(picture.line).unwrap();
        assert_eq!(px(line.width), 57.0);
        assert_eq!(px(line.height), 3.0);
        let colour = Color::srgba(0.2, 0.3, 0.4, 0.9);
        for (node, background, tick) in world
            .query_filtered::<
                (&Node, &BackgroundColor, Option<&RangeScaleTick>),
                Or<(With<ReticleLine>, With<RangeScaleTick>)>,
            >()
            .iter(world)
        {
            assert_eq!(background.0, colour);
            if let Some(tick) = tick {
                assert_eq!(tick.major, tick.range % 600.0 == 0.0);
                assert_eq!(px(node.width), if tick.major { 31.0 } else { 17.0 });
            }
        }
        let mut labels = 0;
        for (node, font, text_colour) in world.query::<(&Node, &TextFont, &TextColor)>().iter(world)
        {
            assert_eq!(px(node.left), 40.0);
            assert_eq!(px(node.top), -11.0);
            assert!(matches!(font.font_size, FontSize::Px(13.0)));
            assert_eq!(text_colour.0, colour);
            labels += 1;
        }
        assert_eq!(labels, 3, "each major interval has one numbered graduation");
    }

    /// The owner's one-device-pixel hairline fixture must occupy one physical row on-grid, with zero-mark centre
    /// within half a pixel of the sight projection on both axes. MEASURED through Bevy's headless
    /// taffy rounding (`crate::gpu_less_default_plugins`), across window/UI scales and subpixel lay
    /// steps. Intent cursor, bore dot and held aim dot must centre on their own projections in
    /// that same frame: one coordinate system for the player.
    ///
    /// Mutants: a thickness in `Val::Px` (two rows at 2x, none or two at a fractional scale); the
    /// window's scale without `UiScale`; a camera-logical point written to `Val::Px` unconverted
    /// (the mark slides off the sight line by the `UiScale` ratio); stale scale or placement
    /// ordering (the first layout after a mid-run scale or pose change must already be correct).
    #[test]
    fn every_mark_is_one_physical_pixel_at_every_scale() {
        use crate::settings::UiScalePercent;
        use bevy::window::WindowResolution;

        /// Window scale factors — fixture data: an ordinary display, two fractional desktop
        /// scalings and a retina 2x.
        const WINDOW_SCALES: [f32; 5] = [1.0, 1.25, 1.5, 2.0, 1.0];
        /// Lay poses per scale pair, each this far (rad) in pitch and yaw from the last: about a
        /// quarter of a physical pixel on this fixture's optic, so the marks cross pixel fractions.
        const LAY_POSES: u16 = 8;
        const LAY_STEP: Vec2 = Vec2::new(1.1e-4, 1.7e-4);
        /// Slack (px) on the half-pixel bound for f32 round-off through projection and layout.
        const ROUND_OFF: f32 = 1e-3;

        let ui_scales = [UiScalePercent::min(), 100, 115, UiScalePercent::max()]
            .map(|percent| UiScalePercent(percent).factor());

        let mut app = App::new();
        app.add_plugins(crate::gpu_less_default_plugins(Some(Window {
            resolution: WindowResolution::new(VIEWPORT.x, VIEWPORT.y)
                .with_scale_factor_override(1.0),
            ..default()
        })))
        .insert_resource(fixture_look())
        .insert_resource(SightMode::Gunner)
        .insert_resource(Ranging { range: DIALED })
        .insert_resource(UiFonts {
            hud: Handle::default(),
            body: Handle::default(),
        })
        .add_systems(
            Startup,
            (
                spawn_ranging_reticle,
                spawn_intent_reticle,
                spawn_optic_mask,
            ),
        )
        .init_resource::<CommittedAim>()
        .init_resource::<Assets<OpticMaskMaterial>>()
        .add_plugins((mount_placement, crate::aim::tests::mount_layout_fixture));
        // `App::run` normally drives plugin finish/cleanup; a bare `update()` loop must do it.
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();

        let world = app.world_mut();
        // The camera parks at the gun node looking down −Z; the gun lays away from it.
        world.spawn((
            Camera3d::default(),
            Projection::Perspective(PerspectiveProjection {
                fov: FOV,
                ..default()
            }),
            Transform::from_translation(GUN),
            PlayerView,
        ));
        let gun = world.spawn(Transform::from_translation(GUN)).id();
        let muzzle = world
            .spawn((table(), Transform::from_translation(GUN)))
            .id();
        let hull = world.spawn((Hull, Transform::IDENTITY)).id();
        let tank = world
            .spawn((
                Controlled,
                crate::command::TankCommand::default(),
                Rig {
                    hull,
                    turret: hull,
                    gun,
                    muzzle,
                },
            ))
            .id();
        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);

        app.update();
        for window_scale in WINDOW_SCALES {
            let world = app.world_mut();
            world
                .query::<&mut Window>()
                .single_mut(world)
                .unwrap()
                .resolution
                .set_scale_factor_override(Some(window_scale));
            for ui_scale in ui_scales {
                app.insert_resource(UiScale(ui_scale));
                for pose in 0..LAY_POSES {
                    let lay = LAY_STEP * f32::from(pose);
                    let rotation = Quat::from_rotation_y(lay.x) * Quat::from_rotation_x(lay.y);
                    let world = app.world_mut();
                    let gun_pose = Transform {
                        translation: GUN,
                        rotation,
                        ..default()
                    };
                    world.entity_mut(gun).insert(gun_pose);
                    world.entity_mut(muzzle).insert(gun_pose);
                    // Fixture: a resolved point along the sight, with an identity hull so local
                    // and world coordinates coincide. The empty collider tree leaves the bore at sky.
                    let point = GUN + sight_line(rotation, table().superelevation(DIALED)) * 100.0;
                    world.resource_mut::<CommittedAim>().set(tank, point);
                    world
                        .get_mut::<crate::command::TankCommand>(tank)
                        .unwrap()
                        .aim = Some(crate::command::AimIntent::World(point));
                    // The first frame after changing scale and pose must already lay out correctly.
                    app.update();

                    let world = app.world_mut();
                    let case =
                        format!("window scale {window_scale}, UiScale {ui_scale}, pose {pose}");
                    let sight = sight_line(
                        world
                            .get::<GlobalTransform>(gun)
                            .expect("the gun node carries a pose")
                            .rotation(),
                        table().superelevation(DIALED),
                    );
                    let (camera, view) = world
                        .query_filtered::<(&Camera, &GlobalTransform), With<PlayerView>>()
                        .single(world)
                        .expect("the fixture's one player view");
                    let physical_projection = |point| {
                        camera
                            .world_to_viewport(view, point)
                            .expect("the fixture's marker projects in front of the camera")
                            * camera
                                .target_scaling_factor()
                                .expect("the camera renders to the window")
                    };
                    let sight_point = physical_projection(GUN + sight);
                    let intent_point = physical_projection(point);
                    let bore_point =
                        physical_projection(GUN + rotation * Vec3::NEG_Z * *crate::aim::MAX_RANGE);

                    let mut drawn = 0;
                    for (node, transform, visibility, zero) in world
                        .query_filtered::<(
                            &ComputedNode,
                            &UiGlobalTransform,
                            &Visibility,
                            Has<ReticleLine>,
                        ), Or<(With<ReticleLine>, With<RangeScaleTick>)>>()
                        .iter(world)
                    {
                        if *visibility == Visibility::Hidden {
                            assert!(!zero, "{case}: the zero mark is on screen");
                            continue;
                        }
                        let height = node.size().y;
                        let top = transform.translation.y - height / 2.0;
                        assert!(
                            height == 1.0 && top == top.round(),
                            "{case}: a mark must lay out one physical row, got {height} px tall \
                             from y = {top}",
                        );
                        if zero {
                            let off = (transform.translation - sight_point).abs();
                            assert!(
                                off.max_element() <= 0.5 + ROUND_OFF,
                                "{case}: the zero mark's row must hold the sight line's physical \
                                 projection {sight_point} to half a pixel, off by {off}",
                            );
                        }
                        drawn += 1;
                    }
                    assert!(
                        drawn > 4,
                        "{case}: the hairline must be measured on the scale, not on a stray mark \
                         — only {drawn} marks were drawn",
                    );
                    let (intent, visibility) = world
                        .query_filtered::<(&UiGlobalTransform, &Visibility), With<IntentReticle>>()
                        .single(world)
                        .unwrap();
                    assert_eq!(*visibility, Visibility::Visible);
                    let intent = ("intent", intent.translation);
                    let [bore, aim] = crate::aim::tests::laid_out_indicator_centres(world);
                    let mut errors = Vec::new();
                    for ((kind, actual), expected) in [intent, bore, aim].into_iter().zip([
                        intent_point,
                        bore_point,
                        intent_point,
                    ]) {
                        let off = (actual - expected).abs();
                        if off.max_element() > 0.5 + ROUND_OFF {
                            errors.push(format!("{kind} centre {actual} misses physical projection {expected} by {off}"));
                        }
                    }
                    assert!(errors.is_empty(), "{case}: {}", errors.join("; "));
                }
            }
        }
    }

    /// Fixture hull far from origin and off-level: wrong-frame composition or dropped translation
    /// must move the projection instead of cancelling. Never a law.
    const HULL_ORIGIN: Vec3 = Vec3::new(137.0, 4.6, -512.0);
    const HULL_ATTITUDE: Vec3 = Vec3::new(0.7, 0.11, 0.23);
    /// The gun mount in the hull's frame: the elevation pivot, well above the hull origin, so a
    /// bearing measured from the hull origin instead reads a different angle.
    const MOUNT: Vec3 = Vec3::new(0.0, 2.2171, -1.100);
    /// The gun's lay under the hull (turret yaw, elevation) and the superelevation dialed onto it.
    const LAY: Vec2 = Vec2::new(0.35, 0.08);
    const LOB: f32 = 0.02;

    fn hull() -> Affine3A {
        Affine3A::from_rotation_translation(
            Quat::from_euler(
                EulerRot::YXZ,
                HULL_ATTITUDE.x,
                HULL_ATTITUDE.y,
                HULL_ATTITUDE.z,
            ),
            HULL_ORIGIN,
        )
    }

    /// Hull → turret yaw → gun elevation; world attitude with bore −Z and trunnion +X.
    fn gun_rotation() -> Quat {
        Quat::from_mat3a(
            &(hull().matrix3
                * Mat3A::from_quat(Quat::from_rotation_y(LAY.x))
                * Mat3A::from_quat(Quat::from_rotation_x(LAY.y))),
        )
    }

    /// A committed point at `offset` (yaw, pitch) radians off the gun's sight line, `range` metres
    /// from the mount, hull-local — the exact form `super::drive_gunner_aim` publishes, and on the
    /// margin's own circle the exact form its clamp saturates to.
    fn intent_at(offset: Vec2, range: f32) -> Vec3 {
        let (yaw, pitch) = yaw_pitch_of(
            hull()
                .inverse()
                .transform_vector3(sight_line(gun_rotation(), LOB)),
        );
        MOUNT + hull_local_dir(yaw + offset.x, pitch + offset.y) * range
    }

    /// Shipped `camera::blended_look` blends sight/committed intent; shipped [`optic_glass`] places
    /// the glass. Neither law is reimplemented.
    struct OpticPicture {
        camera: Camera,
        pose: GlobalTransform,
        viewport: Vec2,
        sight: Vec3,
        margin: f32,
    }

    impl OpticPicture {
        /// `intent` is the hull-local committed point; `None` is a tank with nothing committed.
        fn new(viewport: UVec2, intent: Option<Vec3>, k: f32) -> Self {
            Self::through(OPTICS, viewport, intent, k)
        }

        /// Derive projection and sight bound from one `optics` and viewport, as the client does;
        /// tests can vary only the instrument.
        fn through(optics: Optics, viewport: UVec2, intent: Option<Vec3>, k: f32) -> Self {
            let fov = optics.vertical_fov();
            let aspect = viewport.x as f32 / viewport.y as f32;
            let hull = hull();
            let rotation = gun_rotation();
            let eye = hull.transform_point3(MOUNT);
            let sight = sight_line(rotation, LOB);
            let look = crate::camera::blended_look(&hull, eye, sight, intent, k);
            Self {
                camera: Camera {
                    computed: ComputedCameraValues {
                        clip_from_view: Mat4::perspective_infinite_reverse_rh(fov, aspect, 0.1),
                        target_info: Some(RenderTargetInfo {
                            physical_size: viewport,
                            scale_factor: 1.0,
                        }),
                        ..default()
                    },
                    ..default()
                },
                pose: GlobalTransform::from(
                    Transform::from_translation(eye).looking_to(look, rotation * Vec3::Y),
                ),
                viewport: viewport.as_vec2(),
                sight,
                margin: optic_margin(fov, aspect),
            }
        }

        fn glass(&self) -> OpticGlass {
            optic_glass(&self.camera, &self.pose, self.sight)
                .expect("the shipped placement answers for a camera with a viewport")
        }

        /// The camera's own axis — screen centre, which is what the cutout must NOT be pinned to.
        fn axis(&self) -> Vec3 {
            self.pose.rotation() * Vec3::NEG_Z
        }

        /// Where a world point falls relative to the glass centre, in the units
        /// `assets/shaders/optic_mask.wgsl` compares against the radius: node UV, stretched by the
        /// node's aspect on x so the hole is a circle at any aspect ratio.
        fn glass_offset(&self, glass: &OpticGlass, world: Vec3) -> Vec2 {
            let uv = self
                .camera
                .world_to_viewport(&self.pose, world)
                .expect("a point in front of the camera projects")
                / self.viewport;
            (uv - glass.centre) * Vec2::new(self.viewport.x / self.viewport.y, 1.0)
        }

        /// Invert the shader metric/projection so tests assert sight angles rather than pixels.
        fn bearing_at(&self, glass: &OpticGlass, offset: Vec2) -> Vec3 {
            let uv = glass.centre + offset / Vec2::new(self.viewport.x / self.viewport.y, 1.0);
            self.camera
                .viewport_to_world(&self.pose, uv * self.viewport)
                .expect("a point on the drawn glass unprojects")
                .direction
                .into()
        }
    }

    /// Fixture viewports: 16:9, ultrawide (width-based circles dwarf it), 4:3, portrait (height is
    /// larger) and square. Never a law.
    const ASPECTS: [UVec2; 5] = [
        UVec2::new(1280, 720),
        UVec2::new(2560, 1080),
        UVec2::new(1024, 768),
        UVec2::new(1080, 1920),
        UVec2::new(1024, 1024),
    ];

    /// The bound's own circle, `phi` radians round it — where `super::drive_gunner_aim`'s clamp
    /// saturates, and the set every containment law below is driven on.
    fn on_the_bound(margin: f32, phi: f32) -> Vec2 {
        Vec2::from_angle(phi) * margin
    }

    /// Rim radius uses only the larger viewport axis. Width alone differs by the aspect in
    /// portrait, drawing a circle smaller than cursor reach.
    #[test]
    fn the_rim_spans_the_larger_axis_whichever_axis_that_is() {
        for viewport in ASPECTS {
            let size = viewport.as_vec2();
            let drawn = OpticPicture::new(viewport, None, 0.5).glass().radius;
            // Height units, the shader's: a circle spanning that fraction of the longer side.
            let span =
                0.5 * crate::sight::look::get().optic_span_fraction.0 * size.max_element() / size.y;
            assert!(
                (drawn - span).abs() < EPS,
                "the rim on a {viewport} viewport is {drawn} of the height, against the {span} its \
                 larger axis spans",
            );
            if size.x < size.y {
                let width_basis =
                    0.5 * crate::sight::look::get().optic_span_fraction.0 * size.x / size.y;
                assert!(
                    drawn > width_basis + EPS,
                    "on a portrait viewport the larger axis is the HEIGHT: a width basis would \
                     draw {width_basis}, and this drew {drawn}",
                );
            }
        }
    }

    /// Cursor travel and drawn glass share one projected span at every aspect and FOV.
    /// The projection must cancel the bound's atan; an absolute-angle bound would fail this.
    #[test]
    fn the_bound_is_the_drawn_rim() {
        for viewport in ASPECTS {
            let size = viewport.as_vec2();
            let aspect = size.x / size.y;
            for field_deg in [62.5_f32, 25.0, 15.625, 7.8125, 5.208_333] {
                let fov = Optics::Magnified {
                    magnification: 2.5,
                    field_deg,
                }
                .vertical_fov();
                let (rim, _) = mask_rim(size);
                let bound = 0.5 * optic_margin(fov, aspect).tan() / (fov / 2.0).tan();
                assert!(
                    (rim - bound).abs() < 1e-6,
                    "on a {viewport} viewport framing {field_deg}° the cursor reaches {bound} of \
                     the height against a {rim} rim",
                );
            }
        }
    }

    /// `radius ± feather` must remain distinct floats and stop short of the centre, or smoothstep
    /// is undefined/blank. Check every aspect and the unplaced material seed.
    #[test]
    fn the_rim_keeps_its_feather_representable() {
        let representable = |radius: f32, feather: f32| {
            feather > 0.0 && radius - feather < radius && radius + feather > radius
        };
        for viewport in ASPECTS {
            let glass = OpticPicture::new(viewport, None, 0.5).glass();
            assert!(
                representable(glass.radius, glass.feather),
                "the {} feather collapses onto the {} radius on a {viewport} viewport",
                glass.feather,
                glass.radius,
            );
            assert!(
                glass.feather < glass.radius,
                "the gradient reaches the glass's own centre on a {viewport} viewport",
            );
        }
        assert!(
            representable(
                OPTIC_GLASS_UNPLACED,
                crate::sight::look::get().optic_feather_fraction.0 * OPTIC_GLASS_UNPLACED
            ),
            "the hidden pre-placement material also needs distinct smoothstep edges",
        );
    }

    /// Glass centre follows the sight line: camera-aligned at `k = 0`, a whole bound behind a
    /// camera locked on boundary intent (screen centre near the rim). A node-centred cutout would
    /// pass only the first case.
    #[test]
    fn the_cutout_tracks_the_sight_line_not_the_screen_centre() {
        let welded = OpticPicture::new(VIEWPORT, Some(intent_at(Vec2::ZERO, 900.0)), 0.0);
        let welded_glass = welded.glass();
        let welded_drift = welded
            .glass_offset(&welded_glass, welded.pose.translation() + welded.sight)
            .length();
        assert!(
            welded_drift < 1e-3 && (welded_glass.centre - Vec2::splat(0.5)).length() < 1e-3,
            "at k = 0 the sight line IS the camera axis, so the glass is centred on the node — the \
             cutout sits {welded_drift} glass-units off the sight line",
        );

        // The camera on the intent, the gun a full bound behind it.
        let led = OpticPicture::new(
            VIEWPORT,
            Some(intent_at(on_the_bound(welded.margin, 0.0), 900.0)),
            1.0,
        );
        let led_glass = led.glass();
        let on_sight = angle_between(led.bearing_at(&led_glass, Vec2::ZERO), led.sight);
        assert!(
            on_sight < EPS,
            "the glass centre must name the gun's sight line, off by {on_sight} rad",
        );
        let screen_centre = led
            .glass_offset(&led_glass, led.pose.translation() + led.axis())
            .length();
        assert!(
            screen_centre > 0.9 * led_glass.radius,
            "with the camera led out to the bound the node's centre lands near the rim — it sits \
             {screen_centre} glass-units out against a {} radius, and anything near zero means the \
             cutout was pinned to the screen",
            led_glass.radius,
        );
    }

    /// The intent stays inside the drawn glass at every blend and aspect. Cursor reach and
    /// glass consume one authored span; the camera blends between the sight line and intent.
    ///
    /// The glass is NOT contained in the viewport, and the intent therefore need not be: the rim
    /// spans the LARGER axis, so on a landscape viewport it runs off the top and bottom, and an
    /// intent near the vertical bound sits past the screen edge with the gun's own elevation stop
    /// the thing that binds first.
    ///
    /// Sample the whole bound from point-blank to scale end, through shipped blend/placement,
    /// measuring in the shader metric.
    #[test]
    fn the_intent_stays_inside_the_drawn_glass_at_every_rung() {
        /// Float slack as a fraction of the rim: the containment is an EXACT equality on the
        /// bound's four axes, and the two sides of it reach the screen by different paths.
        const RIM_SLACK: f32 = 1e-5;

        for viewport in ASPECTS {
            for k in BLEND_SAMPLES {
                let margin = OpticPicture::new(viewport, None, k).margin;
                for step in 0..24 {
                    let phi = step as f32 * std::f32::consts::TAU / 24.0;
                    for range in [45.0, 900.0, 4000.0] {
                        let point = intent_at(on_the_bound(margin, phi), range);
                        let picture = OpticPicture::new(viewport, Some(point), k);
                        let glass = picture.glass();
                        let world = hull().transform_point3(point);
                        let out = picture.glass_offset(&glass, world).length();
                        assert!(
                            out <= glass.radius * (1.0 + RIM_SLACK),
                            "on a {viewport} viewport at k = {k} the intent on the bound ({phi} \
                             rad round, {range} m out) drew {out} glass-units from the centre, \
                             past the {} radius",
                            glass.radius,
                        );
                    }
                }
            }
        }
    }
}
