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
use crate::camera::GunnerCameraPlaced;
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
        // Both reprojections run after the camera's pose is final for the frame, and after
        // propagation: the graticule anchors on the VIEW gun's own `GlobalTransform` as well as
        // reprojecting through the camera. Every input is render-rate — the mouse (`intent`,
        // Update) and that VIEW gun pose (blended by `interpolate_servos` in Update, and what the
        // gunner camera itself is placed from) — so the reprojections are clean by construction, no
        // aliasing.
        .add_systems(
            PostUpdate,
            (update_intent_reticle, update_ranging_reticle)
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
/// centre outside scheme A. All hidden until shown in the optic.
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
/// line. Only in scheme A does the anchor coincide with screen centre — there the camera IS bolted
/// to this sight line (`camera::gunner_camera`), so anchoring here is identity; in every other
/// scheme the camera rides the intent or the player's look and the scale correctly lags with the gun.
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
                    // A mark past the viewport's edge is HIDDEN, never clamped onto it: schemes B
                    // and C put no bound on the lead, so the sight line does leave the screen, and
                    // `world_to_viewport` answers off-screen coordinates for anything in front of
                    // the camera (it only fails behind the near plane / past the far plane).
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

    use super::*;

    /// The fixture's viewport, in px.
    const VIEWPORT: UVec2 = UVec2::new(1280, 720);
    /// The optic's magnified vertical FOV (rad) — fixture data off the seed vehicle's authored gunner
    /// view, so one milliradian of lay is several pixels here and a mark placed on the bore instead
    /// of the sight line lands tens of pixels out.
    const FOV: f32 = 0.12;
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
    /// the test puts it, and a camera the test poses per scheme (bolted to the sight line for A, off
    /// it for B–E). Nothing here re-implements the placement law — the shipped system runs.
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

        /// Pose the camera at the gun mount (where every gunner-scheme camera parks) looking along
        /// `look` — the schemes differ only in what that look is.
        fn look_along(&mut self, look: Quat) {
            self.world
                .entity_mut(self.camera)
                .insert(GlobalTransform::from(Transform {
                    translation: GUN,
                    rotation: look,
                    ..default()
                }));
        }

        /// Scheme A's camera, built exactly as `camera::gunner_camera` builds it: parked at the gun
        /// node, looking along the sight line, up = the node's own +Y.
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

    /// **The ruling: the range bar rides the GUN, not the view.** Hold the camera still — outside
    /// scheme A it tracks the player's intent or look, not the gun — and elevate the gun. Every mark
    /// must travel by exactly the lay change; a graticule anchored to the camera's forward axis (the
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
    /// Measured with the camera OFF the sight line (schemes B–E), so a camera-anchored placement
    /// cannot pass by coincidence.
    #[test]
    fn the_zero_mark_is_the_sight_line_a_superelevation_below_the_bore() {
        let mut picture = SightPicture::new(Quat::from_rotation_y(0.03));
        // The camera leads the gun, as scheme E's lock to the intent cursor leaves it.
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

    /// **Scheme A is untouched.** Its camera is bolted to the same sight line
    /// (`camera::gunner_camera`), so anchoring the graticule to the gun is identity there: the zero
    /// mark holds the viewport centre and every graduation lands where the camera-anchored law this
    /// replaces put it, recomputed here from the camera pose alone.
    #[test]
    fn the_bolted_scheme_a_picture_is_unchanged() {
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
                "scheme A must be a no-op: the {range} m graduation moved from {was} to {mark}",
            );
            checked += 1;
        }
        assert!(
            checked > 4,
            "only {checked} graduations were drawn to compare"
        );
    }

    /// **The graticule hides rather than smears.** Schemes B and C bound the lead by nothing, so the
    /// sight line does leave the viewport — off the side (still in front of the camera, which
    /// `world_to_viewport` answers with off-screen coordinates rather than an error) and behind it.
    /// A mark that cannot be drawn where it belongs is not drawn at all, and outside the optic
    /// nothing is drawn at all.
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
