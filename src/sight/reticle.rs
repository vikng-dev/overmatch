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

use crate::aim::CommittedAim;
use crate::camera::{CameraKickApplied, GunnerCameraPlaced};
use crate::damage::ControlledTank;
use crate::firecontrol::{RangeTable, Ranging};
use crate::overlay::{self, Overlay, Overlays};
use crate::spec::ViewKind;
use crate::state::GameplaySet;
use crate::tank::{Controlled, Hull, Rig, TankViews};
use crate::ui_font::UiFonts;

use super::{SightMode, view_available};

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

/// The ranging reticle's static horizontal reference line, held on the sight centre. The moving range
/// scale slides behind it; whichever graduation the line crosses is the dialed range.
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
        .add_systems(
            Startup,
            (
                spawn_intent_reticle,
                spawn_view_death_overlay,
                spawn_toast,
                spawn_range_readout,
                spawn_ranging_reticle,
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
        // The intent cursor reprojects through the gunner camera, so it runs after the camera's pose
        // is final for the frame. Both inputs are render-rate — `intent` (mouse, Update) and the
        // camera pose (which reads the VIEW gun's `GlobalTransform`, blended by
        // `interpolate_servos` in Update) — so the reprojection is clean by construction, no
        // aliasing.
        .add_systems(
            PostUpdate,
            (update_intent_reticle, update_ranging_reticle)
                .in_set(GameplaySet)
                .after(TransformSystems::Propagate)
                .after(GunnerCameraPlaced)
                // After the hit-kick has displaced the camera's rendered pose, so the reticles
                // reproject through the kicked view and the whole sight picture jolts together on a
                // hit. Vacuous edge in SP/headless (the kick set is net-client-only, empty there).
                .after(CameraKickApplied),
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

/// Spawn the ranging reticle: the static centre line (held on the sight centre via a flex box, the
/// same idiom as the white centre dot) and the pool of range graduations (200 m steps, majors
/// numbered in hundreds of metres). All hidden until shown in the optic.
fn spawn_ranging_reticle(mut commands: Commands, fonts: Res<UiFonts>) {
    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|parent| {
            parent.spawn((
                ReticleLine,
                Node {
                    width: Val::Px(96.0),
                    height: Val::Px(2.0),
                    ..default()
                },
                BackgroundColor(RETICLE_COLOR),
                Visibility::Hidden,
            ));
        });

    let mut range = 200.0_f32;
    while range <= 4000.0 {
        let major = (range as i32) % 400 == 0;
        let width = if major { 24.0 } else { 12.0 };
        let mut tick = commands.spawn((
            RangeScaleTick { range, major },
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(width),
                height: Val::Px(2.0),
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

/// Slide the range scale so each graduation sits at `θ(dialed) − θ(range)` above the sight centre: the
/// dialed range lands on the [`ReticleLine`], nearer ranges above it, farther below, the whole scale
/// riding up with the gun as range is dialed out. Reprojected through the gunner camera (after it has
/// placed itself this frame), so it shares the rendered pose; hidden outside the optic. Reads the laid
/// weapon's table — the main gun for now — which is the per-ammo ballistic scale.
fn update_ranging_reticle(
    mode: Res<SightMode>,
    ranging: Res<Ranging>,
    controlled: Query<&Rig, With<Controlled>>,
    tables: Query<&RangeTable>,
    camera: Single<(&Camera, &GlobalTransform)>,
    mut line: Query<&mut Visibility, (With<ReticleLine>, Without<RangeScaleTick>)>,
    mut ticks: Query<(&RangeScaleTick, &mut Node, &mut Visibility), Without<ReticleLine>>,
) {
    let gunner = *mode == SightMode::Gunner;
    if let Ok(mut visibility) = line.single_mut() {
        *visibility = if gunner {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }

    let table = controlled
        .single()
        .ok()
        .and_then(|rig| tables.get(rig.muzzle).ok());
    let (camera, cam_transform) = *camera;
    let rot = cam_transform.rotation();
    let forward = rot * Vec3::NEG_Z;
    let right = rot * Vec3::X;

    for (tick, mut node, mut visibility) in &mut ticks {
        let Some(table) = table.filter(|_| gunner) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        // Angle above centre = θ(dialed) − θ(this mark); rotate the sight line up by it about the
        // camera's right axis (so the scale is screen-vertical regardless of hull roll) and reproject.
        let angle = table.superelevation(ranging.range) - table.superelevation(tick.range);
        let dir = Quat::from_axis_angle(right, angle) * forward;
        match camera.world_to_viewport(cam_transform, cam_transform.translation() + dir) {
            Ok(screen) => {
                let half = if tick.major { 12.0 } else { 6.0 };
                node.left = Val::Px(screen.x - half);
                node.top = Val::Px(screen.y - 1.0);
                *visibility = Visibility::Visible;
            }
            Err(_) => *visibility = Visibility::Hidden,
        }
    }
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
    // The overlay's `Visibility` lives on the full-screen node; its prompt `Text` on the child.
    let (Ok(mut vis), Ok(mut text)) = (overlay_vis.single_mut(), label.single_mut()) else {
        return;
    };

    let (active_view, other_view, other_label) = match *mode {
        SightMode::ThirdPerson => (ViewKind::Commander, ViewKind::Gunner, "gunner optic"),
        SightMode::Gunner => (ViewKind::Gunner, ViewKind::Commander, "third-person"),
    };

    // The active view's crewman is down — the standalone condition for wanting this overlay. Gated on a
    // controlled tank existing (no station to be dead without one).
    let crewman_down = has_controlled && !view_available(&controlled, &views, active_view);

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

    match overlays {
        // Net client: DECLARE presence only; the shared reconciler owns visibility from the fully-
        // declared set (suppressed under any higher overlay). We must not also write `Visibility` here
        // — that would double-write it and read a not-yet-reconciled set.
        Some(mut overlays) => overlays.declare(Overlay::ViewDead, crewman_down),
        // Single-player: no authority — draw whenever the active crewman is down.
        None => {
            vis.set_if_neq(if crewman_down {
                Visibility::Visible
            } else {
                Visibility::Hidden
            });
        }
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
