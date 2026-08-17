//! Gunner sight *presentation*: the HUD widgets that draw the sight picture.
//!
//! Everything here is view-layer — it reads the sight's state (mode, the shared
//! [`aim::CommittedAim`], the dialed [`Ranging`]) and the rendered camera pose, and writes only UI
//! nodes. It never authors a gun command, so the seam against [`super`]'s aim law is clean: this
//! module could be deleted and the tank would still shoot where it was told.
//!
//! The interface is [`plugin`] (mount the whole layer) plus [`Toast`], the one widget the input half
//! needs to raise ("X unavailable" on a refused view switch).

use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::aim::CommittedAim;
use crate::camera::{GUNNER_FOV_FALLBACK, GunnerCameraPlaced, view_fov};
use crate::damage::ControlledTank;
use crate::firecontrol::{RangeTable, Ranging};
use crate::overlay::{self, Overlay, Overlays};
use crate::spec::ViewKind;
use crate::state::GameplaySet;
use crate::tank::{Controlled, Hull, Rig, TankViews, ViewNode};
use crate::ui_font::UiFonts;

use super::{SightMode, sight_line, view_available};

/// The on-screen intent cursor — the marker the gun chases. It moves immediately with the mouse
/// (position control) and drifts back to centre as the gun's lay catches up.
#[derive(Component)]
struct IntentReticle;

/// Full-screen black overlay shown when the active view's crewman is dead, plus a center prompt
/// telling the player to switch to the other view. Hidden when the view is alive.
#[derive(Component)]
struct ViewDeathOverlay;

/// The prompt text inside the [`ViewDeathOverlay`] — its own (child) entity, so the overlay's
/// `Visibility` (on the parent) and this `Text` are written separately.
#[derive(Component)]
struct ViewDeathText;

/// Seconds a refusal toast stays up.
const TOAST_SECONDS: f32 = 2.0;

/// A brief on-screen message — used when a view switch is *refused* (the target view's crewman is
/// down), so the silent Lshift no-op gets a reason. Ticks down in `update_toast`.
///
/// The whole interface is [`Toast::show`]: the input half raises a message, this module owns when it
/// appears and for how long. Fields stay private so the lifetime rule has one home.
#[derive(Resource, Default)]
pub(super) struct Toast {
    message: String,
    remaining: f32,
}

impl Toast {
    pub(super) fn show(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.remaining = TOAST_SECONDS;
    }
}

/// The toast's text node (upper-centre); shown while [`Toast::remaining`] > 0.
#[derive(Component)]
struct ToastText;

/// HUD dialed-range readout, hidden outside gunner view.
#[derive(Component)]
struct RangeReadout;

/// The ranging reticle's horizontal reference line — the graticule's ZERO mark, placed on the gun's
/// sight line. The range scale slides behind it; the graduation it crosses is the dialed range.
#[derive(Component)]
struct ReticleLine;

/// One moving range-scale graduation.
#[derive(Component)]
struct RangeScaleTick {
    range: f32,
    major: bool,
}

/// Mount the presentation layer: spawn every HUD node once, then keep them in step with the sight.
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Toast>()
        // The optic surround is the sight's one custom-shaded widget: a full-screen node whose
        // fragment punches the glass out of it.
        .add_plugins(UiMaterialPlugin::<OpticMaskMaterial>::default())
        .add_systems(
            Startup,
            (
                spawn_intent_reticle,
                spawn_view_death_overlay,
                spawn_toast,
                spawn_range_readout,
                spawn_ranging_reticle,
                spawn_optic_mask,
            ),
        )
        .add_systems(
            Update,
            (
                // On the net client this DECLARES `Overlay::ViewDead` (+ its prompt text) and leaves
                // the visibility swap to the shared `overlay::apply_overlay_visibility` reconciler — so
                // its declaration joins the `Declare` phase and its apply reads the fully-reconciled
                // set (the ordering fix). In single-player (no `Overlays`) it still sets visibility
                // itself. `Declare` is unconfigured in single-player, so it imposes nothing there.
                update_view_death_overlay.in_set(overlay::OverlaySet::Declare),
                update_toast,
                update_range_readout,
            )
                .chain()
                // After `super::toggle_sight`, so a refused switch this frame shows its reason. Only
                // that toggle is player input (gated on the cursor); the overlay, toast, and range
                // readout are presentation and keep updating with the cursor free — hence no
                // `PlayerInputSet` here.
                .after(super::toggle_sight)
                .in_set(GameplaySet),
        )
        // Both reprojections run after the camera's pose is final for the frame, and after
        // propagation: the graticule anchors on the VIEW gun's own `GlobalTransform` as well as
        // reprojecting through the camera. Every input is render-rate — the mouse (`intent`,
        // Update) and that VIEW gun pose (blended by `interpolate_servos` in Update, and what the
        // gunner camera itself is placed from) — so the reprojections are clean by construction, no
        // aliasing.
        .add_systems(
            PostUpdate,
            (
                update_intent_reticle,
                update_ranging_reticle,
                update_optic_mask,
            )
                .in_set(GameplaySet)
                .after(TransformSystems::Propagate)
                .after(GunnerCameraPlaced),
        );
}

fn spawn_intent_reticle(mut commands: Commands) {
    commands.spawn((
        IntentReticle,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(8.0),
            height: Val::Px(8.0),
            border_radius: BorderRadius::MAX,
            ..default()
        },
        BackgroundColor(Color::srgba(1.0, 0.7, 0.1, 0.9)),
        Visibility::Hidden,
    ));
}

/// The full-screen black overlay + center prompt, shown when the active view's crewman is dead.
/// The prompt tells the player to press Lshift to switch to the other view (if its crewman is
/// alive). Solid black — "your crewman's eyes are gone" (design §7a, view-death model).
fn spawn_view_death_overlay(mut commands: Commands, fonts: Res<UiFonts>) {
    commands
        .spawn((
            ViewDeathOverlay,
            // `OverlayNode(ViewDead)` stamps the one-scrim contract's lowest z via its hook (in BOTH
            // single-player and net — the hook always runs): the view-death black sits BELOW the death
            // screen, so whole-crew death (Death latched) can never let this opaque black occlude "YOU
            // DIED" — the spawn-order bug this redesign fixes. On the net client the marker ALSO hands
            // this node's visibility to `overlay::apply_overlay_visibility`, which hard-suppresses it
            // whenever a higher overlay owns the scrim; the z is the belt to that brace.
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
            BackgroundColor(Color::BLACK),
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            parent.spawn((
                ViewDeathText,
                Text::new(""),
                TextFont {
                    // SemiBold: a full-screen crew-death prompt.
                    font: fonts.hud.clone().into(),
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.4, 0.3)),
            ));
        });
}

/// The refusal-toast text node: a centred banner in the upper third, hidden until a refused switch
/// raises it. Its own entity carries both `Text` and `Visibility`, so `update_toast` writes one query.
fn spawn_toast(mut commands: Commands, fonts: Res<UiFonts>) {
    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            position_type: PositionType::Absolute,
            top: Val::Percent(30.0),
            justify_content: JustifyContent::Center,
            ..default()
        })
        .with_children(|parent| {
            parent.spawn((
                ToastText,
                Text::new(""),
                TextFont {
                    // SemiBold: a centred refusal banner.
                    font: fonts.hud.clone().into(),
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(Color::srgb(1.0, 0.75, 0.3)),
                Visibility::Hidden,
            ));
        });
}

/// The dialed-range readout, parked bottom-left; populated/shown only in the optic.
fn spawn_range_readout(mut commands: Commands, fonts: Res<UiFonts>) {
    commands.spawn((
        RangeReadout,
        Text::new(""),
        TextFont {
            // SemiBold: an all-caps gunnery readout ("RANGE ... m").
            font: fonts.hud.clone().into(),
            font_size: FontSize::Px(16.0),
            ..default()
        },
        TextColor(Color::srgba(1.0, 0.8, 0.3, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(24.0),
            left: Val::Px(24.0),
            ..default()
        },
        Visibility::Hidden,
    ));
}

/// Show the dialed range in the optic so the player can read and correct their estimate; hidden in
/// third-person (where scroll is the camera dolly, not ranging).
fn update_range_readout(
    mode: Res<SightMode>,
    ranging: Res<Ranging>,
    mut readout: Query<(&mut Text, &mut Visibility), With<RangeReadout>>,
) {
    let Ok((mut text, mut visibility)) = readout.single_mut() else {
        return;
    };
    if *mode == SightMode::Gunner {
        *text = Text::new(format!("RANGE {} m", ranging.range as i32));
        *visibility = Visibility::Visible;
    } else {
        *visibility = Visibility::Hidden;
    }
}

/// Reticle graticule colour — amber, grouping it with the other gunnery readouts.
const RETICLE_COLOR: Color = Color::srgba(1.0, 0.8, 0.3, 0.85);

/// Bar thickness of every graticule mark (px). Marks are centred on their screen point, so half of
/// this is the vertical placement offset.
const MARK_THICKNESS: f32 = 2.0;

/// Bar width (px) of the zero mark ([`ReticleLine`]) and of the two graduation weights.
const LINE_WIDTH: f32 = 96.0;
const MAJOR_TICK_WIDTH: f32 = 24.0;
const MINOR_TICK_WIDTH: f32 = 12.0;

/// Bar width of one graduation, by weight.
fn tick_width(major: bool) -> f32 {
    if major {
        MAJOR_TICK_WIDTH
    } else {
        MINOR_TICK_WIDTH
    }
}

/// Spawn the ranging reticle: the zero mark and the pool of range graduations (200 m steps, majors
/// numbered in hundreds of metres). Every mark is absolutely positioned — the whole graticule is
/// placed against the gun's sight line each frame ([`update_ranging_reticle`]), which is not screen
/// centre at any rung of `sight::GunnerBlend`. All hidden until shown in the optic.
fn spawn_ranging_reticle(mut commands: Commands, fonts: Res<UiFonts>) {
    commands.spawn((
        ReticleLine,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(LINE_WIDTH),
            height: Val::Px(MARK_THICKNESS),
            ..default()
        },
        BackgroundColor(RETICLE_COLOR),
        Visibility::Hidden,
    ));

    let mut range = 200.0_f32;
    while range <= 4000.0 {
        let major = (range as i32) % 400 == 0;
        let width = tick_width(major);
        let mut tick = commands.spawn((
            RangeScaleTick { range, major },
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(width),
                height: Val::Px(MARK_THICKNESS),
                ..default()
            },
            BackgroundColor(RETICLE_COLOR),
            Visibility::Hidden,
        ));
        if major {
            // Label rides the tick: an absolute child offsets from the tick's own top-left.
            tick.with_children(|parent| {
                parent.spawn((
                    Text::new(format!("{}", (range as i32) / 100)),
                    TextFont {
                        // Regular: a tiny reticle graduation number (12px).
                        font: fonts.body.clone().into(),
                        font_size: FontSize::Px(12.0),
                        ..default()
                    },
                    TextColor(RETICLE_COLOR),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(width + 5.0),
                        top: Val::Px(-7.0),
                        ..default()
                    },
                ));
            });
        }
        range += 200.0;
    }
}

/// Place one graticule mark centred on `screen`, or hide it when the mark has nowhere to go.
fn place_mark(node: &mut Node, visibility: &mut Visibility, screen: Option<Vec2>, width: f32) {
    match screen {
        Some(screen) => {
            node.left = Val::Px(screen.x - width / 2.0);
            node.top = Val::Px(screen.y - MARK_THICKNESS / 2.0);
            *visibility = Visibility::Visible;
        }
        None => *visibility = Visibility::Hidden,
    }
}

/// Slide the range scale so each graduation sits at `θ(dialed) − θ(range)` above the gun's SIGHT
/// LINE: the dialed range lands on the [`ReticleLine`], nearer ranges above it, farther below, the
/// whole graticule riding with the gun as it lays and as range is dialed out. Hidden outside the
/// optic. Reads the laid weapon's table — the main gun for now — which is the per-ammo ballistic
/// scale.
///
/// **The graticule is anchored to the gun, not to the camera.** Its zero is the sight line
/// (`super::sight_line`: the gun's lay minus the dialed superelevation), which is the line the shell
/// arcs back down onto and so the only line the dialed-range mark can name. That is NOT the green
/// bore dot, which is the barrel axis and by design rides the same superelevation ABOVE the sight
/// line. The anchor coincides with screen centre only at `sight::GunnerBlend`'s `0` rung; above it
/// the camera rides toward the intent and the scale correctly lags with the gun.
///
/// The sight line is read off the VIEW gun node — the render-smoothed chain (`interpolate_servos`,
/// `Update`), the same node the optic camera bolts to and the same discipline the bore dot follows
/// (`aim::update_bore_indicator`): the sim chain steps at tick rate and the graticule would stutter
/// against the rendered view. Reprojected after the gunner camera has placed itself this frame, so
/// mark and view share one pose.
fn update_ranging_reticle(
    mode: Res<SightMode>,
    ranging: Res<Ranging>,
    controlled: Query<&Rig, With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    gun: Query<&GlobalTransform>,
    tables: Query<&RangeTable>,
    camera: Single<(&Camera, &GlobalTransform)>,
    mut line: Query<(&mut Node, &mut Visibility), (With<ReticleLine>, Without<RangeScaleTick>)>,
    mut ticks: Query<(&RangeScaleTick, &mut Node, &mut Visibility), Without<ReticleLine>>,
) {
    let (camera, cam_transform) = *camera;
    // Everything the graticule needs or nothing at all: outside the optic, before the rig / range
    // table / view gun bind, or with no viewport to project into, every mark hides together.
    let mark = (*mode == SightMode::Gunner)
        .then(|| {
            let rig = controlled.single().ok()?;
            let table = tables.get(rig.muzzle).ok()?;
            let gun = gun
                .get(ViewNode::resolve(view_nodes.get(rig.gun).ok(), rig.gun))
                .ok()?;
            let viewport = camera.logical_viewport_rect()?;
            let dialed = table.superelevation(ranging.range);
            let sight = sight_line(gun.rotation(), dialed);
            // The sight's own origin, which every gunner camera is parked at, so a mark's bearing is
            // all that decides its screen place.
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
                    // runs out to 4 km, far beyond the optic's reach, and `world_to_viewport`
                    // answers off-screen coordinates for anything in front of the camera (it only
                    // fails behind the near plane / past the far plane).
                    .filter(|screen| viewport.contains(*screen))
            })
        })
        .flatten();

    // The zero mark, on the sight line itself: `θ(dialed) − θ(dialed)` is zero by construction, so
    // the line and the dialed graduation are one place and the graticule cannot come apart.
    if let Ok((mut node, mut visibility)) = line.single_mut() {
        let screen = mark.as_ref().and_then(|mark| mark(ranging.range));
        place_mark(&mut node, &mut visibility, screen, LINE_WIDTH);
    }
    for (tick, mut node, mut visibility) in &mut ticks {
        let screen = mark.as_ref().and_then(|mark| mark(tick.range));
        place_mark(&mut node, &mut visibility, screen, tick_width(tick.major));
    }
}

/// How opaque the surround outside the glass is. `1.0` is what looking down an optic actually looks
/// like; lower dims the world around the sight picture instead of blanking it.
const OPTIC_SURROUND_OPACITY: f32 = 1.0;

/// The drawn circle's DIAMETER, as a fraction of the viewport's LARGER axis. `MASK_SPAN_FRACTION`
/// and `sight::OPTIC_RADIUS_FRACTION` are read off the same axis, so at equal values the cursor's
/// bound IS this rim; a smaller span here would draw a glass the cursor can leave, which
/// [`mask_rim`] asserts against.
const MASK_SPAN_FRACTION: f32 = 0.9;

/// Half-width of the rim's feather, as a fraction of its OWN radius: a broad gradient rather than
/// an anti-alias, which is what reads as a blurred surround. Far enough from `0` that
/// `radius ± feather` stays representable (the shader's `smoothstep` across two edges collapsed
/// onto one float is undefined) and far enough from `1` that the gradient cannot reach the centre.
const MASK_EDGE_FEATHER: f32 = 0.10;

/// The cursor's angular bound carried into the mask's own units — a fraction of the viewport HEIGHT
/// — through the perspective divide the glass itself is measured through.
fn projected_bound(fov: f32, aspect: f32) -> f32 {
    0.5 * super::optic_margin(fov, aspect).tan() / (fov / 2.0).tan()
}

/// The drawn rim for a viewport framing `fov`: `(radius, feather half-width)`, both as fractions of
/// the viewport HEIGHT — the shader's units.
///
/// **The rim must contain the bound**: a smaller radius would draw a glass the cursor can leave.
fn mask_rim(viewport: Vec2, fov: f32) -> (f32, f32) {
    // The larger axis as a comparison, never as an assumption about which one it is: in height
    // units a landscape viewport frames off its width and a portrait one off its height, through
    // this one line.
    let radius = 0.5 * MASK_SPAN_FRACTION * viewport.max_element() / viewport.y;
    debug_assert!(
        radius >= projected_bound(fov, viewport.x / viewport.y) - 1e-6,
        "the mask draws a {radius} rim inside the bound the cursor is clamped to, so the intent \
         can leave the glass",
    );
    (radius, MASK_EDGE_FEATHER * radius)
}

/// The glass radius before [`update_optic_mask`] has measured one: wider than the node's own
/// diagonal at any aspect, so the surround is fully open.
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

/// The optic surround: a full-screen node whose fragment shader punches the glass out of it, so the
/// hole is a true circle at any centre and any aspect ratio (which is why it is a shader and not a
/// stack of rectangles — the centre leaves the node's own centre at every blend but `0`).
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

/// Marks the one full-screen mask node, so [`update_optic_mask`] can find its material and its
/// visibility.
#[derive(Component)]
struct OpticMask;

/// Spawn the optic surround once, hidden. It sits on the overlay ladder's own sight-furniture rung
/// (`overlay::Overlay::SIGHT_FURNITURE_Z`), below the graticule and the intent cursor it surrounds
/// and below every overlay that has to cover the sight picture whole.
fn spawn_optic_mask(mut commands: Commands, mut materials: ResMut<Assets<OpticMaskMaterial>>) {
    commands.spawn((
        OpticMask,
        MaterialNode(materials.add(OpticMaskMaterial {
            // Placed by `update_optic_mask` before it is ever shown; a fully open glass is the
            // harmless pre-placement state.
            params: OpticMaskParams {
                glass: Vec4::new(
                    0.5,
                    0.5,
                    OPTIC_GLASS_UNPLACED,
                    MASK_EDGE_FEATHER * OPTIC_GLASS_UNPLACED,
                ),
                surround: Vec4::new(0.0, 0.0, 0.0, OPTIC_SURROUND_OPACITY),
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

/// Where the optic's glass lands on screen, measured through the camera's ACTUAL projection.
///
/// The centre is the gun's `sight` line reprojected — neither the viewport's centre nor the camera
/// axis, which coincide with it only at `sight::GunnerBlend`'s `0` rung; above that the glass
/// slides as the gun lags, which is the whole point of the knob. It may land OUTSIDE the rect and is
/// passed through unclamped: the visible arc is still the right arc, and clamping would drag the
/// glass off the sight line. Only a sight line BEHIND the camera has no answer, and
/// `world_to_viewport` reports exactly that case (it answers off-rect coordinates for anything in
/// front) — there the mask stands down rather than drawing at a garbage coordinate.
///
/// The rim is pure viewport ([`mask_rim`]), and the cursor's bound is the same fraction of the same
/// axis carried through this same projection, so the reachable circle IS the drawn glass at any
/// aspect. `fov` is the field the optic frames, the bound's other term.
fn optic_glass(
    camera: &Camera,
    cam_transform: &GlobalTransform,
    sight: Vec3,
    fov: f32,
) -> Option<OpticGlass> {
    let viewport = camera.logical_viewport_size()?;
    // Height is the unit every measurement below is in, so a viewport without one has no answer —
    // the same stand-down a sight line behind the camera gets, rather than a NaN radius (a target
    // resized to nothing, which a minimized window is).
    if viewport.y <= 0.0 {
        return None;
    }
    let eye = cam_transform.translation();
    // Sample the ray a whole eye-distance out rather than one metre out. The projection is
    // scale-invariant along a ray, so the reach names no bearing of its own — but f32 spacing at
    // the eye's own magnitude is what quantizes `eye + dir`, and a unit step off a mount standing
    // a kilometre from the world origin loses enough of the direction to move the measured centre
    // a fifth of a pixel. Scaling the step with the eye holds that error at one f32 epsilon of
    // angle anywhere on the map.
    let reach = eye.length().max(1.0);
    let centre = camera
        .world_to_viewport(cam_transform, eye + sight * reach)
        .ok()?;
    let (radius, feather) = mask_rim(viewport, fov);
    Some(OpticGlass {
        centre: centre / viewport,
        radius,
        feather,
    })
}

/// Put the glass on the gun's sight line and show the surround; hidden outside the optic, and
/// whenever the sight picture has no place on screen at all.
///
/// Reads the same VIEW gun node and the same [`sight_line`] the graticule and the optic camera do,
/// after the camera has placed itself this frame — the mask, the zero mark and the view are one
/// pose, so the glass cannot drift against the marks inside it.
fn update_optic_mask(
    mode: Res<SightMode>,
    ranging: Res<Ranging>,
    controlled: Query<&Rig, With<Controlled>>,
    views: Query<&TankViews, With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    gun: Query<&GlobalTransform>,
    tables: Query<&RangeTable>,
    camera: Single<(&Camera, &GlobalTransform)>,
    mut materials: ResMut<Assets<OpticMaskMaterial>>,
    mut mask: Query<(&MaterialNode<OpticMaskMaterial>, &mut Visibility), With<OpticMask>>,
) {
    let Ok((node, mut visibility)) = mask.single_mut() else {
        return;
    };
    let (camera, cam_transform) = *camera;
    let glass = (*mode == SightMode::Gunner)
        .then(|| {
            let rig = controlled.single().ok()?;
            let gun = gun
                .get(ViewNode::resolve(view_nodes.get(rig.gun).ok(), rig.gun))
                .ok()?;
            let theta = tables
                .get(rig.muzzle)
                .map_or(0.0, |table| table.superelevation(ranging.range));
            // The same field the camera's projection was set to and the cursor's bound was taken
            // against, from the same authored optic.
            optic_glass(
                camera,
                cam_transform,
                sight_line(gun.rotation(), theta),
                view_fov(&views, ViewKind::Gunner, GUNNER_FOV_FALLBACK),
            )
        })
        .flatten();

    let Some(glass) = glass else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    if let Some(mut material) = materials.get_mut(&node.0) {
        material.params.glass = glass.centre.extend(glass.radius).extend(glass.feather);
    }
    visibility.set_if_neq(Visibility::Visible);
}

/// Place the intent cursor at the reprojection of the committed aim point — a resolved point on
/// the world in BOTH regimes (a third-person commit resumed on entry, or the optic's own resolve),
/// so its true screen position is exact at any range; no bearing-only shortcut, which would place a
/// near floor aim too high by the mount parallax. As the gun (and so the camera/sight line) catches
/// up, this drifts back to screen centre; hidden outside gunner view.
///
/// Reads the shared [`aim::CommittedAim`] (republished by `super::drive_gunner_aim` earlier this frame
/// in `BeforeFixedMainLoop`) and the gunner camera's pose (which reads the VIEW gun's
/// `GlobalTransform`, blended by `interpolate_servos` in `Update`) — a pure function of the committed
/// intent and the camera, no aliasing.
fn update_intent_reticle(
    mode: Res<SightMode>,
    committed: Res<CommittedAim>,
    camera: Single<(&Camera, &GlobalTransform)>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    hull: Query<&GlobalTransform, With<Hull>>,
    mut reticle: Query<(&mut Node, &mut Visibility), With<IntentReticle>>,
) {
    let Ok((mut node, mut visibility)) = reticle.single_mut() else {
        return;
    };
    if *mode != SightMode::Gunner {
        *visibility = Visibility::Hidden;
        return;
    }
    let Ok((tank, rig)) = controlled.single() else {
        *visibility = Visibility::Hidden;
        return;
    };
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };
    let Some(local) = committed.get(tank) else {
        *visibility = Visibility::Hidden;
        return;
    };
    let (camera, cam_transform) = *camera;

    let point = hull.affine().transform_point3(local);

    match camera.world_to_viewport(cam_transform, point) {
        Ok(screen) => {
            node.left = Val::Px(screen.x - 4.0);
            node.top = Val::Px(screen.y - 4.0);
            *visibility = Visibility::Visible;
        }
        Err(_) => *visibility = Visibility::Hidden,
    }
}

/// Show/hide the black overlay + prompt when the active view's crewman is dead. The prompt tells
/// the player to press Lshift to switch to the other view if its crewman is alive; if both are
/// dead, the prompt says so (the tank is effectively dead — 0 living crew imminent).
///
/// On the NET client this participates in the overlay authority: it runs in
/// [`overlay::OverlaySet::Declare`] and only DECLARES `Overlay::ViewDead` presence (+ refreshes the
/// prompt text), leaving the visibility swap to the shared `overlay::apply_overlay_visibility`
/// reconciler that runs AFTER `Declare`. That split is the ordering fix: the one-scrim decision reads a
/// fully-declared set, so this black is suppressed entirely whenever a higher overlay (the death screen
/// above all, but also the menu / connect screen) owns the scrim — whole-crew death shows "YOU DIED",
/// not this black. In single-player the `Overlays` resource is absent (`Option` is `None`) and this
/// system sets the node's visibility itself, standalone as before: crewman down → black + prompt.
fn update_view_death_overlay(
    mode: Res<SightMode>,
    controlled: ControlledTank,
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
    // controlled tank existing (no station to be dead without one).
    let crewman_down = has_controlled && !view_available(&controlled, &views, active_view);

    // Net client: DECLARE presence only; the shared reconciler owns visibility from the fully-declared
    // set (suppressed under any higher overlay). We must not also write `Visibility` here — that would
    // double-write it and read a not-yet-reconciled set.
    //
    // The declaration is deliberately made BEFORE the node lookup below and gated on nothing: presence
    // is a fact about the crew, not about the view hierarchy. Were it to sit behind the lookup, a frame
    // that failed to find the node would skip `declare` entirely and the set would LATCH the previous
    // answer rather than clear it — `declare` is absolute, so it must be reached unconditionally to
    // self-heal.
    let single_player = match overlays {
        Some(mut overlays) => {
            overlays.declare(Overlay::ViewDead, crewman_down);
            false
        }
        None => true,
    };

    // The overlay's `Visibility` lives on the full-screen node; its prompt `Text` on the child.
    let (Ok(mut vis), Ok(mut text)) = (overlay_vis.single_mut(), label.single_mut()) else {
        return;
    };

    // Refresh the prompt whenever the active crewman is down (identical in both modes); a hidden node's
    // stale text is harmless and re-derived here before it can be shown again.
    if crewman_down {
        let other_available = view_available(&controlled, &views, other_view);
        *text = Text::new(if other_available {
            format!("Crewman down — [Lshift] for {other_label}")
        } else {
            "All view crew down".to_string()
        });
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
    let Ok((mut text, mut visibility)) = label.single_mut() else {
        return;
    };
    if toast.remaining > 0.0 {
        toast.remaining -= time.delta_secs();
        *text = Text::new(toast.message.clone());
        *visibility = Visibility::Visible;
    } else if *visibility != Visibility::Hidden {
        *visibility = Visibility::Hidden;
    }
}

#[cfg(test)]
mod tests {
    use bevy::camera::{ComputedCameraValues, RenderTargetInfo};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::Affine3A;

    use super::*;
    use crate::sight::{GUNNER_BLEND_LADDER, hull_local_dir, optic_margin, yaw_pitch_of};
    use crate::spec::Optics;

    /// The fixture's viewport, in px.
    const VIEWPORT: UVec2 = UVec2::new(1280, 720);
    /// The instrument every fixture below looks through — fixture data, a middling gunnery sight.
    const OPTICS: Optics = Optics::Magnified {
        magnification: 2.5,
        field_deg: 25.0,
    };
    /// The field it frames (rad), through the one conversion the client uses: one milliradian of lay
    /// is a pixel here, so a mark placed on the bore instead of the sight line lands tens out.
    const FOV: f32 = OPTICS.vertical_fov();
    /// Where the gun node stands: far from the world origin, so a placement that composes the wrong
    /// frame moves instead of cancelling.
    const GUN: Vec3 = Vec3::new(137.0, 4.6, -512.0);
    /// The dialed range (m) — on a graduation, so the zero mark and one tick must coincide.
    const DIALED: f32 = 800.0;
    /// Angular tolerance (rad) for a law asserted through the projection round trip: ~0.6 px here.
    const EPS: f32 = 1e-4;

    /// A superelevation that RISES with range across the whole graticule, so every graduation
    /// interpolates rather than saturating on an end row.
    fn table() -> RangeTable {
        RangeTable::test_rows(&[(0.0, 0.0), (1000.0, 0.008), (4000.0, 0.045)])
    }

    /// The angle between two directions, via `atan2` on the cross/dot pair — never `acos` of a dot
    /// product, which loses most of the f32 mantissa near 1, and these are angles that should be 0.
    fn angle_between(a: Vec3, b: Vec3) -> f32 {
        a.cross(b).length().atan2(a.dot(b))
    }

    fn px(val: Val) -> f32 {
        match val {
            Val::Px(px) => px,
            other => panic!("a graticule mark is placed in px, got {other:?}"),
        }
    }

    /// The sight picture as the client draws it: the shipped graticule pool, a gun node laid where
    /// the test puts it, and a camera the test poses where the blend would put it (on the sight line
    /// at `k = 0`, off it above). Nothing here re-implements the placement law — the shipped system
    /// runs.
    struct SightPicture {
        world: World,
        camera: Entity,
        gun: Entity,
        line: Entity,
        /// The spawned graduation pool, as `(entity, range)`.
        ticks: Vec<(Entity, f32)>,
    }

    impl SightPicture {
        fn new(lay: Quat) -> Self {
            let mut world = World::new();
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

        /// The superelevation the dialed range asks for — the fixture's table, read through the
        /// shipped lookup.
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

        /// The screen point the zero mark is centred on, or `None` where it is hidden — the
        /// graticule's only other state.
        fn zero_mark(&self) -> Option<Vec2> {
            self.placed(self.line)
        }

        /// The graduations, as `(range, mark)` pairs.
        fn graduations(&self) -> Vec<(f32, Option<Vec2>)> {
            self.ticks
                .iter()
                .map(|&(entity, range)| (range, self.placed(entity)))
                .collect()
        }

        fn placed(&self, entity: Entity) -> Option<Vec2> {
            let visibility = self.world.get::<Visibility>(entity)?;
            if *visibility == Visibility::Hidden {
                return None;
            }
            let node = self.world.get::<Node>(entity)?;
            Some(Vec2::new(
                px(node.left) + px(node.width) / 2.0,
                px(node.top) + MARK_THICKNESS / 2.0,
            ))
        }

        /// The bearing a screen point names, read back through the camera — so every law below is
        /// stated in angles the sight owns, not in pixel offsets.
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

        /// How far `dir` stands above the camera's forward IN THE PLANE THE GRATICULE STACKS IN
        /// (about the camera's right axis). A rotation about that axis turns the camera-local
        /// `(y, −z)` pair rigidly, so this reads back the placement angle exactly, wherever the
        /// sight line sits on the screen.
        fn elevation(&self, dir: Vec3) -> f32 {
            let local = self.camera_rotation().inverse() * dir;
            local.y.atan2(-local.z)
        }

        /// Where the OLD, camera-anchored law put a mark: the camera's own forward pitched up by
        /// `angle` about its right axis. Scheme A must still land exactly here.
        fn camera_anchored(&self, angle: f32) -> Vec2 {
            let rotation = self.camera_rotation();
            let dir = Quat::from_axis_angle(rotation * Vec3::X, angle) * (rotation * Vec3::NEG_Z);
            let camera = self.world.get::<Camera>(self.camera).expect("a camera");
            let transform = self
                .world
                .get::<GlobalTransform>(self.camera)
                .expect("a camera pose");
            camera
                .world_to_viewport(transform, transform.translation() + dir)
                .expect("the camera-anchored mark projects")
        }
    }

    /// **The ruling: the range bar rides the GUN, not the view.** Hold the camera still — above
    /// `k = 0` it tracks the player's intent, not the gun — and elevate the gun. Every mark must
    /// travel by exactly the lay change; a graticule anchored to the camera's forward axis (the
    /// behaviour this replaces) does not move at all.
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

    /// **The zero mark names the SIGHT LINE, and the bore stands a superelevation above it.** The
    /// sight line is what the shell arcs back down onto at the dialed range, so it is where the
    /// dialed graduation belongs; the green bore dot is the barrel axis and sits θ higher by design.
    /// Pinning the bar's centre on the bore would put every mark that whole angle high.
    ///
    /// Measured with the camera OFF the sight line (any `k` above 0), so a camera-anchored
    /// placement cannot pass by coincidence.
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

    /// **The scale's spacing is the range table's own.** Each graduation stands `θ(dialed) − θ(its
    /// range)` above the sight line: nearer ranges above the zero mark, farther below, the spacing
    /// tightening exactly as the lob does.
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

    /// **The `k = 0` picture is untouched.** There the camera is welded to the same sight line
    /// (`camera::gunner_camera`), so anchoring the graticule to the gun is identity: the zero mark
    /// holds the viewport centre and every graduation lands where the camera-anchored law this
    /// replaces put it, recomputed here from the camera pose alone.
    #[test]
    fn the_welded_picture_is_unchanged() {
        /// Placement tolerance in px — sub-pixel, so the two laws are the same picture.
        const PIXEL: f32 = 0.05;

        let mut picture =
            SightPicture::new(Quat::from_rotation_y(0.4) * Quat::from_rotation_x(0.07));
        picture.bolt_to_sight_line();
        picture.run();

        let centre = VIEWPORT.as_vec2() / 2.0;
        let zero = picture.zero_mark().expect("the zero mark is on screen");
        assert!(
            zero.distance(centre) < PIXEL,
            "with the camera bolted to the sight line the zero mark is screen centre, got {zero} \
             against {centre}",
        );

        let table = table();
        let mut checked = 0;
        for (range, mark) in picture.graduations() {
            let Some(mark) = mark else { continue };
            let angle = table.superelevation(DIALED) - table.superelevation(range);
            let was = picture.camera_anchored(angle);
            assert!(
                mark.distance(was) < PIXEL,
                "k = 0 must be a no-op: the {range} m graduation moved from {was} to {mark}",
            );
            checked += 1;
        }
        assert!(
            checked > 4,
            "only {checked} graduations were drawn to compare"
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

    /// The optic surround's fixture: a hull far from the origin AND off the level, so a placement
    /// that composed the wrong frame — or dropped the hull's translation — moves instead of
    /// cancelling. Fixture data, never a law.
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

    /// The gun node's world attitude: the hull's, carried through the turret's yaw and the gun's
    /// elevation — the chain whose −Z is the bore and whose +X is the trunnion.
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

    /// The optic as the client assembles it at one blend setting: the shipped
    /// `camera::blended_look` places the camera on the gun's sight line and the committed intent,
    /// and the shipped [`optic_glass`] places the glass. Nothing here re-implements either.
    struct OpticPicture {
        camera: Camera,
        pose: GlobalTransform,
        viewport: Vec2,
        sight: Vec3,
        fov: f32,
        margin: f32,
    }

    impl OpticPicture {
        /// `intent` is the hull-local committed point; `None` is a tank with nothing committed.
        fn new(viewport: UVec2, intent: Option<Vec3>, k: f32) -> Self {
            Self::through(OPTICS, viewport, intent, k)
        }

        /// The same picture seen through an arbitrary instrument — the camera's projection and the
        /// sight's bound both come off the one authored `optics` and the one viewport, exactly as
        /// the client derives them, so a test can change the instrument and nothing else.
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
                fov,
                margin: optic_margin(fov, aspect),
            }
        }

        fn glass(&self) -> OpticGlass {
            optic_glass(&self.camera, &self.pose, self.sight, self.fov)
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

        /// The inverse: the bearing named by a point `offset` glass-units from the glass centre —
        /// the shader's metric run backwards through the same projection, so every law below is
        /// stated in angles the sight owns rather than in pixels.
        fn bearing_at(&self, glass: &OpticGlass, offset: Vec2) -> Vec3 {
            let uv = glass.centre + offset / Vec2::new(self.viewport.x / self.viewport.y, 1.0);
            self.camera
                .viewport_to_world(&self.pose, uv * self.viewport)
                .expect("a point on the drawn glass unprojects")
                .direction
                .into()
        }
    }

    /// The viewports every mask law below is measured at: 16:9, an ultrawide where a circle off the
    /// width dwarfs the screen it is drawn on, 4:3, a PORTRAIT one where the larger axis is the
    /// height, and a square. Fixture data, never a law.
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

    /// **The drawn rim spans the viewport's LARGER axis, whichever axis that is.** Its radius is
    /// pure viewport — no angle enters it — so the one thing that can go wrong is asking the width
    /// for the answer and calling it the larger axis. On a portrait viewport the two differ by the
    /// aspect, and the width basis draws a circle the cursor's own reach would swallow.
    #[test]
    fn the_rim_spans_the_larger_axis_whichever_axis_that_is() {
        for viewport in ASPECTS {
            let size = viewport.as_vec2();
            let drawn = OpticPicture::new(viewport, None, 0.5).glass().radius;
            // Height units, the shader's: a circle spanning that fraction of the longer side.
            let span = 0.5 * MASK_SPAN_FRACTION * size.max_element() / size.y;
            assert!(
                (drawn - span).abs() < EPS,
                "the rim on a {viewport} viewport is {drawn} of the height, against the {span} its \
                 larger axis spans",
            );
            if size.x < size.y {
                let width_basis = 0.5 * MASK_SPAN_FRACTION * size.x / size.y;
                assert!(
                    drawn > width_basis + EPS,
                    "on a portrait viewport the larger axis is the HEIGHT: a width basis would \
                     draw {width_basis}, and this drew {drawn}",
                );
            }
        }
    }

    /// **The travel circle IS the drawn glass, exactly, at every aspect and every field.** Both
    /// sides are the same fraction of the viewport's LARGER half-extent in the camera's projected
    /// space — `sight::OPTIC_RADIUS_FRACTION` reaching it through an `atan` the projection's `tan`
    /// undoes, `MASK_SPAN_FRACTION` spanning it as a diameter — so the equality holds to float
    /// noise rather than approximately, and the `debug_assert` in [`mask_rim`] guards it on every
    /// drawn frame.
    ///
    /// The field cancels out of both sides, so a sight of any power reaches the same rim: a bound
    /// coupled to an absolute angle would move by the 12× this sweep spans.
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
                let (rim, _) = mask_rim(size, fov);
                let bound = projected_bound(fov, aspect);
                assert!(
                    (rim - bound).abs() < 1e-6,
                    "on a {viewport} viewport framing {field_deg}° the cursor reaches {bound} of \
                     the height against a {rim} rim",
                );
            }
        }
    }

    /// **The intent driven round the bound FILLS the glass: it touches the rim on the four axes and
    /// never comes in more than a couple of percent between them.** The property that makes the
    /// drawn circle mean something: a bound off the vertical field alone would leave 44% of the
    /// glass unreachable at 16:9.
    ///
    /// The dip between the axes is the bound's own shape: it is a circle in the sight's yaw/pitch
    /// ANGLES, and the perspective divide maps that to a slightly flattened curve on screen —
    /// MEASURED at 1.6% on the diagonals, entirely INSIDE the rim.
    #[test]
    fn the_bound_fills_the_drawn_glass_at_every_bearing() {
        for viewport in ASPECTS {
            let picture = OpticPicture::new(viewport, None, 0.0);
            let glass = picture.glass();
            for step in 0..24 {
                let phi = step as f32 * std::f32::consts::TAU / 24.0;
                let point = intent_at(on_the_bound(picture.margin, phi), 900.0);
                let reach = picture
                    .glass_offset(&glass, hull().transform_point3(point))
                    .length()
                    / glass.radius;
                assert!(
                    (0.98..=1.000_01).contains(&reach),
                    "on a {viewport} viewport the bound {phi} rad round reached {reach} of the \
                     drawn radius",
                );
            }
        }
    }

    /// **The bound takes the LARGER axis, whichever axis that is.** A bound off the vertical field
    /// alone would be far tighter than the rim on any landscape viewport; one off the width would
    /// be tighter than the rim on a portrait one, where the height is the larger axis.
    ///
    /// Worked instances as fixture data: the TZF 9b frames 25°, reaching 19.53° at 16:9 and 11.28°
    /// on a portrait or square viewport.
    #[test]
    fn the_bound_is_sized_by_the_larger_axis() {
        let vertical = optic_margin(FOV, 1.0);
        for viewport in ASPECTS {
            let size = viewport.as_vec2();
            let aspect = size.x / size.y;
            let margin = optic_margin(FOV, aspect);
            if aspect > 1.0 {
                assert!(
                    margin > vertical,
                    "a {viewport} viewport is wider than it is tall, so the cursor must reach \
                     further than the {vertical} rad the vertical field alone gives",
                );
            } else {
                assert!(
                    (margin - vertical).abs() < EPS,
                    "on a {viewport} viewport the height is the larger axis, so the reach is the \
                     vertical one — {margin} rad against {vertical}",
                );
            }
        }

        let wide = optic_margin(FOV, 16.0 / 9.0);
        assert!(
            (wide.to_degrees() - 19.53).abs() < 5e-3
                && (vertical.to_degrees() - 11.28).abs() < 5e-3,
            "the TZF 9b's 25° reaches {}° at 16:9 and {}° on a square viewport",
            wide.to_degrees(),
            vertical.to_degrees(),
        );
    }

    /// **The rim's feather stays representable at its own radius.** The shader smoothsteps across
    /// `radius ± feather`, which is undefined where the two edges collapse onto one float, and
    /// blank where the gradient reaches the centre. Both ends are checked at every aspect, plus the
    /// unplaced seed the mask spawns holding.
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
                MASK_EDGE_FEATHER * OPTIC_GLASS_UNPLACED
            ),
            "the pre-placement seed is drawn too, on the frame before the mask is measured",
        );
    }

    /// **The glass is cut around the GUN, not around the screen.** Its centre is the sight line
    /// reprojected: welded to the camera axis only at `k = 0`, and with the camera locked to an
    /// intent held out at the bound, displaced by that whole reach, which puts screen centre most
    /// of the way to the rim. A cutout pinned to the node's centre passes the first case and fails
    /// the second.
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

    /// **The intent can never leave the drawn GLASS — at any blend rung and any aspect.** The one
    /// property that makes the mask coherent, and it rests on a SHARED NUMBER:
    /// `super::OPTIC_RADIUS_FRACTION` is the fraction of the larger half-extent the cursor reaches
    /// and `MASK_SPAN_FRACTION` is the fraction of that same axis the circle spans, both in the
    /// camera's projected space, so the reachable circle IS the drawn one at every aspect with no
    /// `min()` and no aspect branch. It touches that rim on all four axes, which is why this is
    /// asserted rather than argued. The blend contributes the rest: it puts the camera on the
    /// segment between the sight line and the intent, so no rung can push the intent past a rim
    /// measured from either end.
    ///
    /// The glass is NOT contained in the viewport, and the intent therefore need not be: the rim
    /// spans the LARGER axis, so on a landscape viewport it runs off the top and bottom, and an
    /// intent near the vertical bound sits past the screen edge with the gun's own elevation stop
    /// the thing that binds first.
    ///
    /// Driven on the bound itself, all the way round it, at ranges from point-blank to the range
    /// scale's end, through the shipped blend and the shipped placement, and measured in the
    /// shader's own metric.
    #[test]
    fn the_intent_stays_inside_the_drawn_glass_at_every_rung() {
        /// Float slack as a fraction of the rim: the containment is an EXACT equality on the
        /// bound's four axes, and the two sides of it reach the screen by different paths.
        const RIM_SLACK: f32 = 1e-5;

        for viewport in ASPECTS {
            for k in GUNNER_BLEND_LADDER {
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

    /// `update_view_death_overlay` must declare `ViewDead` presence even when the overlay node is
    /// absent. `declare` is absolute, so a frame that skipped it would leave the previous answer
    /// LATCHED — the failure this ordering rules out. The node cannot go missing today (it spawns at
    /// `Startup` and never despawns), so this pins the decoupling rather than a live bug.
    #[test]
    fn declaration_survives_a_missing_overlay_node() {
        let mut app = App::new();
        app.init_resource::<SightMode>();
        app.init_resource::<Overlays>();
        // Latch it first: clearing a latched overlay is the direction that used to be skippable.
        app.world_mut()
            .resource_mut::<Overlays>()
            .declare(Overlay::ViewDead, true);
        app.add_systems(Update, update_view_death_overlay);

        // No overlay node, no text child, no controlled tank — every query below the declaration
        // fails to resolve.
        app.update();

        assert!(
            !app.world()
                .resource::<Overlays>()
                .contains(Overlay::ViewDead),
            "no controlled tank means no dead station, and the declaration must reach the set to \
             clear it even though the overlay node is missing"
        );
    }
}
