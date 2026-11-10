//! Render-rate aim authorship/HUD and fixed-clock [`drive_aim_servos`] for every tank/servo.
//! Third person picks through screen centre; `sight::drive_gunner_aim` authors optic intent.
//! RMB free-look re-authors the held bearing; the HUD marks centre, bore and held aim.
//!
//! [`CommittedAim`] holds raw sight-line points hull-locally (ADR-0001: unstabilized lay).
//! Transport follows the view's frame (ADR-0038): world-locked orbit picks travel as
//! [`AimIntent::World`]; hull-anchored optic/free-aim picks travel as [`AimIntent::HullLocal`].
//! The mode-agnostic servo bridge converts either in the hull frame of the consuming tick.

mod data;
pub(crate) use data::{INPUT_RON, MAX_RANGE};

use std::sync::LazyLock;

use avian3d::prelude::{LayerMask, Position, Rotation, SpatialQuery, SpatialQueryFilter};
use bevy::ecs::entity::EntityHashMap;
use bevy::math::Affine3A;
use bevy::prelude::*;
use bevy::ui::UiSystems;

use crate::Layer;
use crate::ballistics::armour_set::LiveArmour;
use crate::camera::GunnerCameraPlaced;
use crate::command::{AimIntent, ParsedTankCommand, TankCommand, TankDeviceSet};
use crate::firecontrol::{RangeTable, lob};
use crate::flow::{Flow, FlowScoped, InMatchSystems, MatchResourceExt};
use crate::sight::{SightMode, SightToggled, in_third_person};
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{
    Controlled, Hull, Rig, ServoCommand, ServoRole, Tank, TankRoot, ViewNode, rig_world_pose,
};
use crate::view::PlayerView;

/// DERIVED per-axis input bound (`command::ParsedTankCommand`): world's diagonal
/// ([`crate::world::VIEW_CAST_MAX_M`]) + [`MAX_RANGE`] sky fallback bounds all legitimate picks
/// with headroom, while keeping poisoned inputs below f32 hull-composition overflow magnitudes.
pub(crate) static AIM_LIMIT: LazyLock<f32> =
    LazyLock::new(|| crate::world::VIEW_CAST_MAX_M + *MAX_RANGE);

/// Distance along an aim ray to the first surface a shell would meet: terrain and scenery on the
/// `Terrain | Armor` mask the live shell marches against (`ballistics::integrate_projectiles`), and
/// every tank's armour volumes from its armour set posed live ([`LiveArmour`]), so the dots predict
/// the shell's truth, tanks included. `own` tank's set is excluded: every aim ray legitimately
/// starts inside or behind its armour (the optic resolve at the gun pivot inside the mantlet, the
/// third-person pick behind own turret, the bore ray at the muzzle inside the barrel's
/// exposed-component volume), and a self-hit would weld the aim to the tank itself. Falls back to
/// `max` on a miss (sky / above the horizon); the CAST itself is clamped to
/// [`crate::world::VIEW_CAST_MAX_M`] — nothing exists between the world diagonal and [`MAX_RANGE`]
/// for a ray to hit, so behavior is identical at a fraction of the parry traversal.
pub(crate) fn aim_distance(
    spatial: &SpatialQuery,
    armour: &LiveArmour,
    ray: Ray3d,
    max: f32,
    own: Entity,
) -> f32 {
    let reach = max.min(crate::world::VIEW_CAST_MAX_M);
    // Terrain and armour outside every tank (a construction) are colliders; a tank's armour parts
    // are not, and answer from their set.
    let scenery = spatial
        .cast_ray(
            ray.origin,
            ray.direction,
            reach,
            true,
            &SpatialQueryFilter::from_mask(
                LayerMask::from(Layer::Terrain) | LayerMask::from(Layer::Armor),
            ),
        )
        .map(|hit| hit.distance);
    let tank = armour
        .cast_ray(ray, reach, |tank| tank != own)
        .map(|hit| hit.distance);
    scenery
        .into_iter()
        .chain(tank)
        .reduce(f32::min)
        .unwrap_or(max)
}

/// Render-rate aim authors (`commit_aim` / `sight::drive_gunner_aim`); fixed-clock
/// `drive_aim_servos` consumes the intention standing at each tick.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AimCommit;

/// HUD: the third-person aiming crosshair — screen centre, which is where [`commit_aim`] casts its
/// pick ray. Held there by its flexbox parent, so it carries no position of its own.
#[derive(Component)]
struct CentreReticle;

/// HUD: where the barrel is actually pointing (lags the reticle) — the gun's reality.
#[derive(Component)]
struct BoreIndicator;

/// HUD: the committed aim point — where the gun is *commanded* to point. Shown only during
/// free-look, since otherwise it sits exactly under the center reticle.
#[derive(Component)]
struct AimIndicator;

pub fn sim_plugin(app: &mut App) {
    LazyLock::force(&MAX_RANGE);
    // In `GameplaySet`, so `drive_servos` (`.after(GameplaySet)`) integrates the fresh targets
    // the same tick. Mode-agnostic and per-tank.
    app.add_systems(FixedUpdate, drive_aim_servos.in_set(GameplaySet));
}

/// Shared hull-local RAW sight-line point, before `drive_aim_servos` adds superelevation.
/// Both modes read/write this memory, so toggles need no seeding handoff (ADR-0001).
///
/// Invariants:
/// - Recirculation: the net input bridge overwrites `command.aim` from buffered input each tick.
///   Both active-mode authors must re-author memory every frame, including RMB hold; otherwise
///   stale sweeps recirculate (MEASURED live period ≈ D+1 ticks). The wire echo is not memory.
/// - Possession: entity-keyed; respawn or SP Tab possession reads no commitment for a new tank.
/// - Single writer: only the outgoing mode commits on a toggle frame. The optic writes in
///   `BeforeFixedMainLoop`; `commit_aim.before(SightToggled)` evaluates before the flip. Without
///   that order a gunner→third flip can pick through a still-gunner `GlobalTransform`: exit re-aim
///   writes `Transform`, propagation is PostUpdate, so the gun's mid-slew lag point overwrites
///   player intent. Incoming mode first writes next frame through its own camera pose.
/// - Zero-input transition is identity: fresh picks resolve world points before hull-local
///   storage, never bare directions (avoiding mount parallax). Their origins differ: a crest
///   can occlude the mount ray but not the elevated camera. `sight::resume_commit` resumes
///   mount-relative yaw/pitch yet re-authors the original point without moving memory, gun or
///   reticle until optic input (or a fresh tank) resolves/stores again.
#[derive(Resource, Default)]
pub(crate) struct CommittedAim(Option<(Entity, Vec3)>);

impl CommittedAim {
    /// Hull-local point, or `None` for empty memory / a different tank (possession invariant).
    pub(crate) fn get(&self, tank: Entity) -> Option<Vec3> {
        self.0.and_then(|(e, p)| (e == tank).then_some(p))
    }

    /// Re-author the held bearing through this frame's hull pose (ADR-0038). Fixed hull-local
    /// storage sweeps the world point as the hull turns, so stopping picks cannot stabilize aim
    /// (ADR-0001), and fresh world naming avoids delivery-age rotation.
    pub(crate) fn as_intent(&self, tank: Entity, hull: Affine3A) -> Option<AimIntent> {
        self.get(tank)
            .map(|held| AimIntent::World(hull.transform_point3(held)))
    }

    /// The active mode commits/rekeys to `tank`, adopting possession on its first write.
    pub(crate) fn set(&mut self, tank: Entity, point: Vec3) {
        self.0 = Some((tank, point));
    }
}

/// The third-person aim commit + HUD dots — client-side: devices → command, and reprojection.
pub fn client_plugin(app: &mut App) {
    crate::flow::require(app, "aim::client_plugin");
    app.init_match_resource::<CommittedAim>()
        .add_systems(OnEnter(Flow::InMatch), spawn_hud)
        .add_systems(
            Update,
            // `CommittedAim` owns why only the outgoing mode may write on a toggle frame.
            commit_aim
                .run_if(in_third_person)
                .before(SightToggled)
                .in_set(AimCommit)
                .in_set(PlayerInputSet)
                .in_set(TankDeviceSet)
                .in_set(GameplaySet),
        )
        // Presentation, not input: no `PlayerInputSet`, so the dot answers the view it is in even
        // with the cursor free. `.after(SightToggled)` puts the hide on the frame of the switch.
        .add_systems(
            Update,
            update_centre_reticle
                .after(SightToggled)
                .in_set(GameplaySet)
                .in_set(InMatchSystems),
        );
    mount_indicator_placement(app);
}

/// Reproject indicators after the camera and UI scale are current, before layout consumes them.
fn mount_indicator_placement(app: &mut App) {
    // Camera propagation/placement and UI scale precede projection, avoiding lag/jitter
    // against the rendered view, most visible at optic zoom.
    app.add_systems(
        PostUpdate,
        (update_bore_indicator, update_aim_indicator)
            .in_set(GameplaySet)
            .after(TransformSystems::Propagate)
            .after(GunnerCameraPlaced)
            .after(UiSystems::Propagate)
            .before(UiSystems::Layout)
            .in_set(InMatchSystems),
    );
}

fn spawn_hud(mut commands: Commands) {
    let look = crate::sight::look::get();
    // Third person is the default view; `update_centre_reticle` hides the centre dot in the optic.
    commands
        .spawn((
            FlowScoped(Flow::InMatch),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .with_children(|parent| {
            parent.spawn((
                CentreReticle,
                Node {
                    width: Val::Px(look.centre.diameter_ui_px.0),
                    height: Val::Px(look.centre.diameter_ui_px.0),
                    border_radius: BorderRadius::MAX,
                    ..default()
                },
                BackgroundColor(look.centre.colour()),
                Visibility::Visible,
            ));
        });

    commands.spawn((
        FlowScoped(Flow::InMatch),
        BoreIndicator,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(look.bore.diameter_ui_px.0),
            height: Val::Px(look.bore.diameter_ui_px.0),
            border_radius: BorderRadius::MAX,
            ..default()
        },
        BackgroundColor(look.bore.colour()),
        Visibility::Hidden,
    ));
    commands.spawn((
        FlowScoped(Flow::InMatch),
        AimIndicator,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(look.aim.diameter_ui_px.0),
            height: Val::Px(look.aim.diameter_ui_px.0),
            border_radius: BorderRadius::MAX,
            ..default()
        },
        BackgroundColor(look.aim.colour()),
        Visibility::Hidden,
    ));
}

/// Third-person centre names `commit_aim`'s ray. The optic picks a bearing off the gun sight
/// line, already marked by graticule zero; another centre would claim a false aim. No tank, no aim.
fn centre_dot_visible(mode: SightMode, has_tank: bool) -> bool {
    has_tank && mode == SightMode::ThirdPerson
}

fn update_centre_reticle(
    mode: Res<SightMode>,
    controlled: Query<(), With<Controlled>>,
    mut reticle: Query<&mut Visibility, With<CentreReticle>>,
) {
    let mut visibility = reticle
        .single_mut()
        .expect("the in-match aim HUD is spawned with exactly one centre reticle");
    visibility.set_if_neq(if centre_dot_visible(*mode, !controlled.is_empty()) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    });
}

/// A controlled, bound hull has a finite tick-truth frame in its root's hierarchy.
/// `GlobalTransform` belongs to presentation and cannot supply aim memory's frame.
fn hull_frame(
    hull: Entity,
    tank: Entity,
    poses: &Query<(&Position, &Rotation)>,
    parents: &Query<&ChildOf>,
    locals: &Query<&Transform>,
) -> Affine3A {
    let (position, rotation) = poses
        .get(tank)
        .expect("a controlled tank with a bound rig carries its physics pose");
    let (hull_position, hull_rotation) = rig_world_pose(
        hull, tank, position.0, rotation.0, parents, locals,
    )
    .expect("a controlled hull remains in its root hierarchy with complete local transforms");
    let affine = Affine3A::from_rotation_translation(hull_rotation, hull_position);
    assert!(
        affine.matrix3.is_finite() && affine.translation.is_finite(),
        "a controlled hull frame is finite before aim commitment"
    );
    affine
}

/// Pick a world point through screen centre (orbit camera is world-locked, ADR-0038), storing
/// its hull-local bearing in [`CommittedAim`]. RMB re-authors [`CommittedAim::as_intent`] instead
/// of picking; with no commitment (first frame / new possession), author nothing.
/// [`CommittedAim`] owns recirculation and `drive_aim_servos` owns the shared optic/third-person lay.
#[expect(
    clippy::too_many_arguments,
    reason = "a system spanning mouse input, the player camera and window, the controlled tank, aim-ray geometry and the committed aim"
)]
fn commit_aim(
    mouse: Res<ButtonInput<MouseButton>>,
    spatial: SpatialQuery,
    armour: LiveArmour,
    camera_query: Query<(&Camera, &GlobalTransform), With<PlayerView>>,
    window: Query<&Window>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    poses: Query<(&Position, &Rotation)>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    mut tank_commands: Query<&mut TankCommand>,
    mut committed: ResMut<CommittedAim>,
) {
    // No local possession is legitimate before spawn and between controlled tanks.
    let Some((tank, rig)) = crate::tank::controlled_one(controlled.single()) else {
        return;
    };
    let hull = hull_frame(rig.hull, tank, &poses, &parents, &locals);
    let to_local = hull.inverse();
    assert!(
        to_local.matrix3.is_finite() && to_local.translation.is_finite(),
        "a controlled hull frame is invertible before aim commitment"
    );
    let mut command = tank_commands
        .get_mut(tank)
        .expect("a controlled tank carries its command state from spawn");

    // Free-look holds memory; an empty/new-tank key is the legitimate pre-first-commit state.
    if mouse.pressed(MouseButton::Right) {
        if let Some(aim) = committed.as_intent(tank, hull)
            // Skip equal writes; restore a bridge-overwritten aim before PostUpdate HUD and
            // next tick's input sample, without at-rest change-detection churn.
            && command.aim != Some(aim)
        {
            command.aim = Some(aim);
        }
        return;
    }

    let (camera, cam_transform) = camera_query
        .single()
        .expect("an active aim commitment has exactly one player camera");
    let window = window
        .single()
        .expect("the player input composition has exactly one window");
    let ray = match camera.viewport_to_world(cam_transform, window.size() / 2.0) {
        Ok(ray) => ray,
        // The camera's target may not have been computed on its first frame.
        Err(bevy::camera::ViewportConversionError::NoViewportSize) => return,
        Err(error) => panic!("an aim camera with a viewport produces a finite ray: {error}"),
    };
    assert!(
        ray.origin.is_finite(),
        "an active aim camera has a finite ray origin"
    );

    let point = ray.get_point(aim_distance(&spatial, &armour, ray, *MAX_RANGE, tank));

    // Store raw sight-line intent; servo-time lob raises the bore above its amber HUD point.
    let aim = Some(AimIntent::World(point));
    // A parked player can re-pick the same point; skip equal writes as in the hold arm.
    if command.aim != aim {
        command.aim = aim;
    }
    committed.set(tank, to_local.transform_point3(point));
}

/// Lay every tank's yaw/pitch servos from one intention, each at its own composed pose.
/// Nested mounts (turret/gun, hull MG) converge independently; `drive_servos` owns slew gates.
/// Apply the primary gun's commanded-range superelevation here so commands/HUD retain raw intent
/// and `drive_servos` stays a generic point-chaser; coax/hull MG share that lob pending per-weapon lay.
/// Convert tagged intent at consumption (ADR-0038): parent-local servo angles and the hull-up
/// elevation plane require this tick's hull frame, regardless of author or view mode.
fn drive_aim_servos(
    tanks: Query<(Entity, &ParsedTankCommand, &Rig, &Position, &Rotation), With<Tank>>,
    tables: Query<&RangeTable>,
    mut servos: Query<(
        Entity,
        &mut ServoCommand,
        &ServoRole,
        &TankRoot,
        Option<&crate::tank::ServoRest>,
    )>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    mut laid: Local<EntityHashMap<LaidTank>>,
) {
    laid.clear();
    for (tank, command, rig, position, rotation) in &tanks {
        let Some(intent) = command.aim() else {
            continue; // no commitment yet — servos hold
        };
        // Use `rig_world_pose`'s physics state; differently stale GlobalTransforms would
        // make client/server lay diverge under maneuver.
        let (hull_position, hull_rotation) =
            rig_world_pose(rig.hull, tank, position.0, rotation.0, &parents, &locals)
                .expect("an assembled tank rig's hull pose composes from its spawn-time hierarchy");
        let hull_affine = Affine3A::from_rotation_translation(hull_rotation, hull_position);
        let to_local = hull_affine.inverse();
        assert!(
            to_local.matrix3.is_finite() && to_local.translation.is_finite(),
            "an authority hull pose is finite and invertible before laying servos"
        );

        let theta = tables
            .get(rig.muzzle)
            .expect("the assembled primary muzzle has its range table from spawn-time weapon data")
            .superelevation(command.range);
        let point = hull_affine.transform_point3(lob(intent.in_hull(&to_local), theta));
        laid.insert(
            tank,
            LaidTank {
                point,
                position: position.0,
                rotation: rotation.0,
            },
        );
    }
    // No entry means no committed aim for this root: retain the servo's target.
    for (servo, mut servo_command, role, root, rest) in &mut servos {
        let Some(tank) = laid.get(&root.0) else {
            continue;
        };
        let (servo_position, frame) = crate::tank::servo_aim_frame(
            servo,
            root.0,
            tank.position,
            tank.rotation,
            rest.expect("a registered servo carries its spawn-built rest rotation"),
            &parents,
            &locals,
        );
        let dir = frame.inverse() * (tank.point - servo_position);
        servo_command.target = match role {
            ServoRole::Yaw => (-dir.x).atan2(-dir.z),
            ServoRole::Pitch => dir.y.atan2((dir.x * dir.x + dir.z * dir.z).sqrt()),
        };
    }
}

/// One tank's laid frame for the servo pass of `drive_aim_servos`: the lobbed world point, the
/// tick-truth root pose its servos compose against.
struct LaidTank {
    point: Vec3,
    position: Vec3,
    rotation: Quat,
}

/// Project `world_point` to the screen and place a HUD dot there (its top-left offset by
/// `half_size` to centre the dot), hiding it when the point is off-screen or behind the camera.
fn place_indicator(
    node: &mut Node,
    visibility: &mut Visibility,
    camera: &Camera,
    cam_transform: &GlobalTransform,
    world_point: Vec3,
    half_size: f32,
    target: &ComputedUiRenderTargetInfo,
) {
    match camera.world_to_viewport(cam_transform, world_point) {
        Ok(screen)
            if camera
                .logical_viewport_rect()
                .expect("a successful viewport projection has the viewport it projected into")
                .contains(screen) =>
        {
            // Camera logical pixels -> physical target pixels -> this node's UI pixels.
            let to_ui = camera
                .target_scaling_factor()
                .expect("a successful viewport projection has the camera's target scale")
                / target.scale_factor();
            let screen = screen * to_ui;
            node.left = Val::Px(screen.x - half_size);
            node.top = Val::Px(screen.y - half_size);
            *visibility = Visibility::Visible;
        }
        Ok(_) | Err(_) => *visibility = Visibility::Hidden,
    }
}

fn update_bore_indicator(
    spatial: SpatialQuery,
    armour: LiveArmour,
    camera_query: Query<(&Camera, &GlobalTransform), With<PlayerView>>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    muzzle: Query<&GlobalTransform>,
    mut indicator: Query<
        (&mut Node, &mut Visibility, &ComputedUiRenderTargetInfo),
        With<BoreIndicator>,
    >,
) {
    let (camera, cam_transform) = camera_query
        .single()
        .expect("the in-match aim HUD has exactly one player camera");
    let (mut node, mut visibility, target) = indicator
        .single_mut()
        .expect("the in-match aim HUD spawns exactly one indicator of each kind");
    let Some((tank, rig)) = crate::tank::controlled_one(controlled.single()) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    // The VIEW muzzle (design §6C): the bore dot must ride the render-smoothed chain — the sim
    // muzzle steps at tick rate since the sim/view split.
    let Ok(muzzle) = muzzle.get(ViewNode::resolve(
        view_nodes.get(rig.muzzle).ok(),
        rig.muzzle,
    )) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };

    // MEASURED during network rig binding: a zeroed muzzle GlobalTransform can make unchecked
    // `forward()` normalization panic. Fallible direction skips that frame.
    let Ok(direction) = Dir3::new(muzzle.rotation() * -Vec3::Z) else {
        return;
    };
    let ray = Ray3d::new(muzzle.translation(), direction);
    let point = ray.get_point(aim_distance(&spatial, &armour, ray, *MAX_RANGE, tank));

    place_indicator(
        &mut node,
        &mut visibility,
        camera,
        cam_transform,
        point,
        crate::sight::look::get().bore.diameter_ui_px.0 / 2.0,
        target,
    );
}

fn update_aim_indicator(
    mouse: Res<ButtonInput<MouseButton>>,
    // Current parentless camera Transform is its world pose (ADR-0003); reproject held aim
    // through it as free-look pans. `commit_aim` picks through GlobalTransform.
    camera_query: Query<(&Camera, &Transform), With<PlayerView>>,
    controlled: Query<(&Rig, &TankCommand), With<Controlled>>,
    hull: Query<&GlobalTransform, With<Hull>>,
    mut indicator: Query<
        (&mut Node, &mut Visibility, &ComputedUiRenderTargetInfo),
        With<AimIndicator>,
    >,
) {
    let (camera, cam_transform) = camera_query
        .single()
        .expect("the in-match aim HUD has exactly one player camera");
    let cam_transform = GlobalTransform::from(*cam_transform);
    let (mut node, mut visibility, target) = indicator
        .single_mut()
        .expect("the in-match aim HUD spawns exactly one indicator of each kind");

    if !mouse.pressed(MouseButton::Right) {
        *visibility = Visibility::Hidden;
        return;
    }

    let Some((rig, command)) = crate::tank::controlled_one(controlled.single()) else {
        *visibility = Visibility::Hidden;
        return;
    };
    let Ok(hull) = hull.get(rig.hull) else {
        *visibility = Visibility::Hidden;
        return;
    };

    // No committed aim yet (before first aim, or free-look from frame one).
    let Some(intent) = command.aim else {
        *visibility = Visibility::Hidden;
        return;
    };

    // The dot marks the intention in whichever frame it was measured; a hull-anchored one rides the
    // RENDERED hull, the pose the player is looking at.
    let world = intent.in_world(&hull.affine());

    place_indicator(
        &mut node,
        &mut visibility,
        camera,
        &cam_transform,
        world,
        crate::sight::look::get().aim.diameter_ui_px.0 / 2.0,
        target,
    );
}

#[cfg(test)]
pub(crate) mod tests {
    /// Aim authorship must expose an internal pose failure before emitting a pick or retaining
    /// an older bearing. A missing local possession is handled before this bound-rig invariant.
    #[test]
    #[should_panic(expected = "a controlled hull frame is finite before aim commitment")]
    fn aim_commit_refuses_a_nonfinite_hull_frame() {
        let mut rig = AimRig::new();
        rig.world.get_mut::<Position>(rig.tank).unwrap().0 = Vec3::splat(f32::NAN);
        rig.run(commit_aim);
    }

    #[test]
    #[should_panic(
        expected = "a controlled hull remains in its root hierarchy with complete local transforms"
    )]
    fn aim_commit_refuses_a_broken_hull_hierarchy() {
        let mut rig = AimRig::new();
        rig.world.entity_mut(rig.hull).remove::<ChildOf>();
        rig.run(commit_aim);
    }

    /// Viewport conversion checks direction, but its finite-origin invariant belongs to the
    /// camera's owner. A poisoned origin must reach neither aim memory nor the input stream.
    #[test]
    #[should_panic(expected = "an active aim camera has a finite ray origin")]
    fn aim_commit_refuses_a_nonfinite_camera_origin() {
        let mut rig = AimRig::new();
        *rig.world.get_mut::<GlobalTransform>(rig.camera).unwrap() =
            GlobalTransform::from(Transform::from_translation(Vec3::splat(f32::NAN)));
        rig.run(commit_aim);
    }

    /// Marker authors own diameter and colour; the spawner must not retain a Rust taste.
    #[test]
    fn hud_marker_sizes_and_colours_come_from_sight_data() {
        use bevy::ecs::system::RunSystemOnce;

        fn check<M: Component>(world: &mut World, look: &crate::sight::look::DotLook) {
            let (node, background) = world
                .query_filtered::<(&Node, &BackgroundColor), With<M>>()
                .single(world)
                .unwrap();
            assert_eq!(node.width, Val::Px(look.diameter_ui_px.0));
            assert_eq!(node.height, Val::Px(look.diameter_ui_px.0));
            assert_eq!(background.0.to_linear(), look.colour().to_linear());
        }

        let mut world = World::new();
        world.run_system_once(spawn_hud).unwrap();
        let look = crate::sight::look::get();
        check::<CentreReticle>(&mut world, &look.centre);
        check::<BoreIndicator>(&mut world, &look.bore);
        check::<AimIndicator>(&mut world, &look.aim);
    }

    use std::collections::VecDeque;
    use std::time::Duration;

    use avian3d::collider_tree::ColliderTrees;
    use bevy::camera::{ComputedCameraValues, RenderTargetInfo};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::input::mouse::AccumulatedMouseMotion;

    use super::*;
    use crate::damage::VolumeOf;
    use crate::firecontrol::{RangeTable, Ranging};
    use crate::sight::drive_gunner_aim;
    use crate::tank::{ServoIndex, ServoRest, ServoSpec, TankServos, drive_servos};

    /// A successfully projected point may still be beyond a viewport edge. The aim and bore
    /// widgets share this placer, so neither may show an off-screen dot or clamp it to an edge.
    #[test]
    fn indicators_hide_points_beyond_the_viewport() {
        // Fixture: a finite perspective camera and points in front, beyond each edge, and behind.
        let camera = Camera {
            computed: ComputedCameraValues {
                clip_from_view: Mat4::perspective_infinite_reverse_rh(
                    std::f32::consts::FRAC_PI_2,
                    16.0 / 9.0,
                    0.1,
                ),
                target_info: Some(RenderTargetInfo {
                    physical_size: UVec2::new(1280, 720),
                    scale_factor: 1.0,
                }),
                ..default()
            },
            ..default()
        };
        let pose = GlobalTransform::default();
        for (point, visible) in [
            (Vec3::new(0.0, 0.0, -10.0), true),
            (Vec3::new(-100.0, 0.0, -10.0), false),
            (Vec3::new(100.0, 0.0, -10.0), false),
            (Vec3::new(0.0, -100.0, -10.0), false),
            (Vec3::new(0.0, 100.0, -10.0), false),
            (Vec3::new(0.0, 0.0, 10.0), false),
        ] {
            if point.z < 0.0 {
                assert!(
                    camera.world_to_viewport(&pose, point).is_ok(),
                    "the regression must exercise projections that succeed even beyond an edge"
                );
            }
            let mut node = Node::default();
            let mut visibility = Visibility::Visible;
            place_indicator(
                &mut node,
                &mut visibility,
                &camera,
                &pose,
                point,
                2.0,
                &ComputedUiRenderTargetInfo::default(),
            );
            assert_eq!(
                visibility,
                if visible {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                },
                "point {point} must only show inside the viewport"
            );
        }
    }

    /// Use the shipped indicator spawner and schedule in the sight's GPU-less layout scene.
    pub(crate) fn mount_layout_fixture(app: &mut App) {
        app.init_resource::<ColliderTrees>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(Startup, spawn_hud);
        mount_indicator_placement(app);
    }

    pub(crate) fn laid_out_indicator_centres(world: &mut World) -> [(&'static str, Vec2); 2] {
        let (bore, visibility) = world
            .query_filtered::<(&UiGlobalTransform, &Visibility), With<BoreIndicator>>()
            .single(world)
            .unwrap();
        assert_eq!(
            *visibility,
            Visibility::Visible,
            "the fixture's bore projects on screen"
        );
        let bore = bore.translation;
        let (aim, visibility) = world
            .query_filtered::<(&UiGlobalTransform, &Visibility), With<AimIndicator>>()
            .single(world)
            .unwrap();
        assert_eq!(
            *visibility,
            Visibility::Visible,
            "the fixture holds a committed aim in free-look"
        );
        [("bore", bore), ("aim", aim.translation)]
    }

    /// Fixture: fixed integration step, seconds.
    const TICK: f32 = 1.0 / 64.0;
    /// Fixture gap, ticks: shipping delay (`net::client::SHIPPING_INPUT_DELAY_TICKS`, 3) plus
    /// authoring frame, without prediction (ADR-0037). Sim code cannot import netcode
    /// (`src/net/tests/net_boundary.rs`); this copy only sets test stress, never an assertion's
    /// expected answer, so drift cannot falsely pass a law.
    const DELIVERY_TICKS: usize = 3 + 1;
    /// Fixture hand rates: aiming tests exercise transport and reference frames, independently of engine power.
    const YAW_SPEED: f32 = 34.0;
    const PITCH_SPEED: f32 = 23.0;

    /// The physics root's world position: far from the origin, so a hull frame composed with
    /// `transform_vector3` where it owes `transform_point3` (or the reverse) moves every number
    /// below instead of cancelling.
    const ROOT: Vec3 = Vec3::new(137.0, 4.0, -512.0);
    /// The hull's offset under the root, so the hull frame's translation is not the root's either.
    const HULL_OFFSET: Vec3 = Vec3::new(0.3, 0.8, -0.4);
    /// Where the third-person camera watches the tank from.
    const EYE: Vec3 = Vec3::new(150.0, 16.0, -496.0);
    /// The superelevation the fixture's muzzle lobs by, at every range (radians). Non-zero so the
    /// lob is never the identity and its frame is under measurement in every law below; large
    /// enough that a lob taken in the wrong frame moves the bore by degrees, not by noise.
    const SUPERELEVATION: f32 = 0.05;

    /// Fixture using `commit_aim` / `sight::drive_gunner_aim`, `drive_aim_servos` and
    /// `tank::drive_servos` at fixture traverse rates; supplies devices, poses and a delivery gap,
    /// not a reimplemented transport. Author against the client-rendered hull, lay against the
    /// authority hull: freeze/step differences expose frame-round-trip noise (ADR-0037).
    /// Turret/gun/muzzle share an origin, excluding rig parallax from transport/servo residuals;
    /// hull and root origins differ, and both are away from world zero.
    struct AimRig {
        world: World,
        tank: Entity,
        hull: Entity,
        turret: Entity,
        gun: Entity,
        camera: Entity,
        /// The delivery gap, as the queue it is: what the servos consume this tick is what the
        /// player authored [`DELIVERY_TICKS`] ticks ago.
        in_flight: VecDeque<Option<AimIntent>>,
        /// What the shipped commit system put on the wire this frame, before the gap swallowed it.
        authored: Option<AimIntent>,
    }

    impl AimRig {
        fn new() -> Self {
            let mut world = World::new();
            let mut time = Time::<()>::default();
            time.advance_by(Duration::from_secs_f32(TICK));
            world.insert_resource(time);
            world.init_resource::<CommittedAim>();
            world.init_resource::<Ranging>();
            world.init_resource::<ButtonInput<MouseButton>>();
            world.init_resource::<AccumulatedMouseMotion>();
            // An empty collider set: every aim ray misses, so `aim_distance` returns the shared sky
            // fallback and the picked point is a pure bearing off the camera.
            world.init_resource::<ColliderTrees>();
            world.spawn(Window::default());

            let camera = world
                .spawn((
                    Camera {
                        computed: ComputedCameraValues {
                            clip_from_view: Mat4::perspective_infinite_reverse_rh(
                                0.9,
                                16.0 / 9.0,
                                0.1,
                            ),
                            target_info: Some(RenderTargetInfo {
                                physical_size: UVec2::new(1280, 720),
                                scale_factor: 1.0,
                            }),
                            ..default()
                        },
                        ..default()
                    },
                    // Readers resolve the declared player view, not all cameras.
                    crate::view::PlayerView,
                    GlobalTransform::default(),
                ))
                .id();

            let hull = world.spawn(Transform::from_translation(HULL_OFFSET)).id();
            let turret = world.spawn(Transform::IDENTITY).id();
            let gun = world.spawn(Transform::IDENTITY).id();
            let muzzle = world
                .spawn((Transform::IDENTITY, RangeTable::test_fixed(SUPERELEVATION)))
                .id();
            let tank = world
                .spawn((
                    Tank,
                    Controlled,
                    // Fixture optic; use the authored degrees-to-radians conversion.
                    crate::tests::tank_views(crate::spec::Optics::Unaided(60.0).vertical_fov()),
                    TankCommand::default(),
                    Position(ROOT),
                    Rotation::default(),
                    Transform::from_translation(ROOT),
                    Rig {
                        hull,
                        turret,
                        gunner_yaw: turret,
                        gun,
                        muzzle,
                    },
                    TankServos::for_count(2),
                ))
                .id();
            world.entity_mut(hull).insert(ChildOf(tank));
            world.entity_mut(turret).insert(ChildOf(hull));
            world.entity_mut(gun).insert(ChildOf(turret));
            world.entity_mut(muzzle).insert(ChildOf(gun));
            // One ballistic volume, so the root carries the `TankVolumes` the servo's requirement
            // gate resolves through; the authored requirement is empty, so effectiveness is full.
            world.spawn(VolumeOf(tank));

            let mut mount = |servo: Entity, role: ServoRole, slot: usize, speed| {
                world.entity_mut(servo).insert((
                    ServoSpec::test_continuous(role, speed),
                    ServoRest(Quat::IDENTITY),
                    ServoIndex(slot),
                    TankRoot(tank),
                    ServoCommand::default(),
                    role,
                ));
            };
            mount(turret, ServoRole::Yaw, 0, YAW_SPEED);
            mount(gun, ServoRole::Pitch, 1, PITCH_SPEED);

            Self {
                world,
                tank,
                hull,
                turret,
                gun,
                camera,
                in_flight: VecDeque::from(vec![None; DELIVERY_TICKS]),
                authored: None,
            }
        }

        /// Set root yaw for authoring or laying, using the pose that pass sees.
        fn stand_at(&mut self, yaw_deg: f32) {
            self.stand_with(Quat::from_rotation_y(yaw_deg.to_radians()));
        }

        /// Set root attitude; off-level hull up differs from world up.
        fn stand_with(&mut self, attitude: Quat) {
            self.world.entity_mut(self.tank).insert(Rotation(attitude));
        }

        /// The hull's world attitude — the frame the servos solve in, and the frame the lob claims.
        fn hull_attitude(&mut self) -> Quat {
            let (tank, hull) = (self.tank, self.hull);
            self.pose_of(hull, tank).1
        }

        /// `rig_world_pose` for one rig node, run as the shipped composer rather than re-derived.
        fn pose_of(&mut self, node: Entity, tank: Entity) -> (Vec3, Quat) {
            self.world
                .run_system_once(
                    move |poses: Query<(&Position, &Rotation)>,
                          parents: Query<&ChildOf>,
                          locals: Query<&Transform>| {
                        let (position, rotation) =
                            poses.get(tank).expect("the root carries a physics pose");
                        rig_world_pose(node, tank, position.0, rotation.0, &parents, &locals)
                            .expect("the node hangs under the root")
                    },
                )
                .expect("the pose probe runs")
        }

        /// World-locked camera view whose centre ray `commit_aim` picks along.
        fn watch(&mut self, target: Vec3) {
            self.world
                .entity_mut(self.camera)
                .insert(GlobalTransform::from(
                    Transform::from_translation(EYE).looking_at(target, Vec3::Y),
                ));
        }

        /// Raw mouse counts select the optic's steering/publish arm rather than hold.
        fn mouse_motion(&mut self, delta: Vec2) {
            self.world.insert_resource(AccumulatedMouseMotion { delta });
        }

        fn right_mouse(&mut self, held: bool) {
            let mut mouse = self.world.resource_mut::<ButtonInput<MouseButton>>();
            if held {
                mouse.press(MouseButton::Right);
            } else {
                mouse.release(MouseButton::Right);
            }
        }

        /// One whole frame across the seam: the shipped author writes against the hull the client
        /// renders, the intention crosses the delivery gap, and the shipped servo bridge plus the
        /// real mechanism lay it against the hull the authority has.
        fn frame(&mut self, rendered_yaw: f32, true_yaw: f32, author: Author) {
            self.stand_at(rendered_yaw);
            match author {
                Author::ThirdPerson => self.run(commit_aim),
                Author::Optic => self.run(drive_gunner_aim),
            }

            self.authored = self.command_aim();
            self.in_flight.push_back(self.authored);
            let delivered = self.in_flight.pop_front().expect("the queue is primed");
            self.world
                .entity_mut(self.tank)
                .get_mut::<TankCommand>()
                .expect("the tank carries a command")
                .aim = delivered;

            self.stand_at(true_yaw);
            self.run(drive_aim_servos);
            self.run(drive_servos);
        }

        /// Lay an intention that is already standing against a hull at `true_yaw`, with no fresh
        /// authoring — the authority's half alone, which is also what a starved input buffer does
        /// with the last command it received.
        fn relay(&mut self, intent: AimIntent, true_yaw: f32) {
            self.relay_with(intent, Quat::from_rotation_y(true_yaw.to_radians()));
        }

        /// [`Self::relay`] against an arbitrary hull attitude.
        fn relay_with(&mut self, intent: AimIntent, attitude: Quat) {
            self.world
                .entity_mut(self.tank)
                .get_mut::<TankCommand>()
                .expect("the tank carries a command")
                .aim = Some(intent);
            self.stand_with(attitude);
            self.run(drive_aim_servos);
            self.run(drive_servos);
        }

        fn run<M, S: bevy::ecs::system::IntoSystem<(), (), M> + 'static>(&mut self, system: S) {
            self.world
                .run_system_once(crate::command::parse_commands)
                .unwrap();
            self.world
                .run_system_once(system)
                .expect("the shipped system runs");
        }

        fn command_aim(&self) -> Option<AimIntent> {
            self.world
                .get::<TankCommand>(self.tank)
                .expect("the tank carries a command")
                .aim
        }

        /// The world place the last third-person pick named — read back off what the commit system
        /// authored, so the test never re-implements the camera ray it is measuring.
        fn picked(&self) -> Vec3 {
            match self.authored.expect("a pick was authored") {
                AimIntent::World(point) => point,
                AimIntent::HullLocal(point) => panic!(
                    "the world-locked orbit view must name a place, got the hull-local {point}"
                ),
            }
        }

        /// The bearing off the hull the turret is COMMANDED to: its parent-local target, degrees.
        fn hull_relative_bearing_deg(&self) -> f32 {
            self.world
                .get::<ServoCommand>(self.turret)
                .expect("the turret carries a servo command")
                .target
                .to_degrees()
        }

        /// The world bearing the turret is COMMANDED to — its parent-local target composed with the
        /// hull heading that target was solved against.
        fn commanded_bearing_deg(&self) -> f32 {
            let (hull_yaw, _, _) = self
                .world
                .get::<Rotation>(self.tank)
                .expect("the root carries a physics rotation")
                .to_euler(EulerRot::YXZ);
            self.hull_relative_bearing_deg() + hull_yaw.to_degrees()
        }

        /// `(azimuth, elevation)` miss in degrees IN `frame`. Hull-frame lay owes zero
        /// azimuth and the commanded superelevation; world-frame answers differ off level.
        /// Use atan2, not acos(dot): near one, acos loses most of f32's mantissa just where
        /// these angles should be zero.
        fn bore_miss_deg(&mut self, target: Vec3, frame: Quat) -> (f32, f32) {
            let (tank, gun) = (self.tank, self.gun);
            let (position, rotation) = self.pose_of(gun, tank);
            let to_frame = frame.inverse();
            let bore = to_frame * (rotation * Vec3::NEG_Z);
            let line = to_frame * (target - position).normalize();
            let azimuth = |v: Vec3| (-v.x).atan2(-v.z);
            let elevation = |v: Vec3| v.y.atan2(v.xz().length());
            (
                (azimuth(bore) - azimuth(line)).to_degrees(),
                (elevation(bore) - elevation(line)).to_degrees(),
            )
        }
    }

    /// Which shipped commit system authors a frame.
    #[derive(Clone, Copy, Debug)]
    enum Author {
        /// `commit_aim` — the world-locked orbit view.
        ThirdPerson,
        /// Hull-anchored `sight::drive_gunner_aim`: hold with still mouse, resolve/publish with motion.
        Optic,
    }

    /// Third-person transport (ADR-0038): one camera look names one world place across hull
    /// headings; a hull-local author would separate fixture picks by 20°. That place must
    /// survive a later turning hull (starved-buffer case), not rotate by its heading change.
    /// A freeze/step rendered hull likewise must not swing the commanded bearing by the whole
    /// step and back. Scope is commanded bearing: the rendered bore still staircases with
    /// the hull and rate-limited mount, outside transport's guarantee (ADR-0038).
    #[test]
    fn a_third_person_pick_names_a_place_whatever_the_hull_is_doing() {
        const TURN_DEG: f32 = 20.0;
        /// The jitter stair's hull yaw rate (degrees/second).
        const OMEGA: f32 = 10.0;
        /// Ticks the interpolated hull holds still before it catches up in one jump.
        const FREEZE_TICKS: u32 = 6;
        let target = ROOT + Vec3::new(0.0, 0.0, -500.0);

        let mut rig = AimRig::new();
        rig.watch(target);
        rig.frame(0.0, 0.0, Author::ThirdPerson);
        let at_rest = rig.picked();
        rig.frame(TURN_DEG, TURN_DEG, Author::ThirdPerson);
        let turned = rig.picked();

        let drift = at_rest.distance(turned);
        assert!(
            drift < 1e-3,
            "one look must name one place: the same camera ray authored {drift:.3} m apart across \
             a {TURN_DEG}° heading change, so the pick was measured against the hull",
        );

        // The value authored before the turn, laid against the turned hull for the whole slew.
        let intent = AimIntent::World(at_rest);
        for _ in 0..256 {
            rig.relay(intent, TURN_DEG);
        }
        let hull = rig.hull_attitude();
        let (azimuth, elevation) = rig.bore_miss_deg(at_rest, hull);
        assert!(
            azimuth.abs() < 0.02,
            "a converged bore must sit on the place the player picked; missing its bearing by \
             {azimuth:.3}° means the intention rode the hull round (a hull-local transport misses \
             by the full {TURN_DEG}° heading change)",
        );
        let lobbed = SUPERELEVATION.to_degrees();
        assert!(
            (elevation - lobbed).abs() < 0.02,
            "the bore must sit exactly the commanded superelevation above the intention — the lob \
             is the only thing entitled to move it off the line of sight — got {elevation:.3}° \
             against {lobbed:.3}°",
        );

        let mut rig = AimRig::new();
        rig.watch(target);
        rig.frame(0.0, 0.0, Author::ThirdPerson);
        let place = rig.picked();
        let mut swing = (f32::MAX, f32::MIN);
        for t in 0..512u32 {
            // Fixture: rendered yaw holds/steps at window boundaries; authority yaw is smooth.
            let shown = (t / FREEZE_TICKS * FREEZE_TICKS) as f32;
            rig.frame(
                OMEGA * shown * TICK,
                OMEGA * t as f32 * TICK,
                Author::ThirdPerson,
            );
            // Past the priming ticks, so every sample is of a fully delivered intention.
            if t >= 64 {
                let bearing = rig.commanded_bearing_deg();
                swing = (swing.0.min(bearing), swing.1.max(bearing));
            }
        }

        let peak_to_peak = swing.1 - swing.0;
        let hull_step = OMEGA * FREEZE_TICKS as f32 * TICK;
        // The offset turret ring translates around the pivot: real parallax sets the floor.
        // DERIVED hull-local transport's square wave is two orders larger on this fixture.
        let parallax = (2.0 * HULL_OFFSET.xz().length() / (place - ROOT).length())
            .atan()
            .to_degrees();
        assert!(
            peak_to_peak < parallax,
            "the commanded bearing must not move while the intention stands still, whatever the \
             client's rendered hull does — got {peak_to_peak:.4}° of swing against the turret \
             ring's own {parallax:.4}° of parallax, and the {hull_step:.3}° square wave a \
             hull-local transport beats out at the clamp's period",
        );
    }

    struct LayFixture {
        app: App,
        roots: [Entity; 2],
        yaws: [Entity; 2],
        pitches: [Entity; 2],
        points: [Vec3; 2],
    }

    impl LayFixture {
        fn new() -> Self {
            let mut app = App::new();
            app.add_plugins(sim_plugin);
            let world = app.world_mut();
            // Fixture: distinct root translations, headings and intended local bearings.
            let positions = [Vec3::new(-20.0, 0.0, 30.0), Vec3::new(15.0, 0.0, -40.0)];
            let rotations = [Quat::from_rotation_y(0.3), Quat::from_rotation_y(-0.7)];
            let points = [Vec3::new(10.0, 4.0, -100.0), Vec3::new(-30.0, -8.0, -50.0)];
            let roots = std::array::from_fn(|i| {
                world
                    .spawn((
                        Position(positions[i]),
                        Rotation(rotations[i]),
                        Transform::IDENTITY,
                    ))
                    .id()
            });
            let hulls: [Entity; 2] =
                std::array::from_fn(|i| world.spawn((ChildOf(roots[i]), Transform::IDENTITY)).id());
            let mut servo = |i: usize, parent, role| {
                world
                    .spawn((
                        ChildOf(parent),
                        Transform::IDENTITY,
                        role,
                        TankRoot(roots[i]),
                        ServoRest(Quat::IDENTITY),
                        ServoCommand::default(),
                    ))
                    .id()
            };
            // Entity order interleaves owners differently in the two servo tables.
            let b_yaw = servo(1, hulls[1], ServoRole::Yaw);
            let a_yaw = servo(0, hulls[0], ServoRole::Yaw);
            let a_pitch = servo(0, a_yaw, ServoRole::Pitch);
            let b_pitch = servo(1, b_yaw, ServoRole::Pitch);
            let yaws = [a_yaw, b_yaw];
            let pitches = [a_pitch, b_pitch];
            for i in 0..2 {
                let muzzle = world.spawn(RangeTable::test_fixed(0.0)).id();
                let command = TankCommand {
                    aim: Some(AimIntent::World(positions[i] + rotations[i] * points[i])),
                    ..default()
                };
                world.entity_mut(roots[i]).insert((
                    Tank,
                    Rig {
                        hull: hulls[i],
                        turret: yaws[i],
                        gunner_yaw: pitches[i],
                        gun: pitches[i],
                        muzzle,
                    },
                    ParsedTankCommand::try_from(command).unwrap(),
                ));
            }
            Self {
                app,
                roots,
                yaws,
                pitches,
                points,
            }
        }

        fn step(&mut self) {
            self.app.world_mut().run_schedule(FixedUpdate);
        }
    }

    /// Every servo consumes its own root's intention and physics frame, independent of entity
    /// order. A single cached hull or first-tank lookup mislays the other owner's axes.
    #[test]
    fn interleaved_tanks_lay_each_servo_from_its_own_root() {
        let mut fixture = LayFixture::new();
        fixture.step();
        for i in 0..2 {
            let point = fixture.points[i];
            let expected_yaw = (-point.x).atan2(-point.z);
            let expected_pitch = point.y.atan2(point.xz().length());
            let world = fixture.app.world();
            assert!(
                (world.get::<ServoCommand>(fixture.yaws[i]).unwrap().target - expected_yaw).abs()
                    < 1e-5
            );
            assert!(
                (world
                    .get::<ServoCommand>(fixture.pitches[i])
                    .unwrap()
                    .target
                    - expected_pitch)
                    .abs()
                    < 1e-5
            );
        }
    }

    /// A tank with no current commitment retains every target, including after a prior committed
    /// update. The other tank keeps laying; stale per-root scratch must not re-author the held one.
    #[test]
    fn no_commitment_retains_targets_after_a_prior_lay() {
        let mut fixture = LayFixture::new();
        fixture.step();
        let held = [(fixture.yaws[0], 0.31), (fixture.pitches[0], -0.42)]; // Fixture held targets.
        fixture
            .app
            .world_mut()
            .entity_mut(fixture.roots[0])
            .insert(ParsedTankCommand::default());
        for (servo, target) in held {
            fixture
                .app
                .world_mut()
                .get_mut::<ServoCommand>(servo)
                .unwrap()
                .target = target;
        }
        fixture
            .app
            .world_mut()
            .get_mut::<ServoCommand>(fixture.yaws[1])
            .unwrap()
            .target = 0.0;
        fixture.step();
        for (servo, target) in held {
            assert_eq!(
                fixture
                    .app
                    .world()
                    .get::<ServoCommand>(servo)
                    .unwrap()
                    .target,
                target
            );
        }
        assert!(
            fixture
                .app
                .world()
                .get::<ServoCommand>(fixture.yaws[1])
                .unwrap()
                .target
                .abs()
                > 0.0
        );
    }

    /// A yaw command is relative to its actual parent frame, including an independently turned
    /// structural yaw. Otherwise the sight repeats its parent's turn and cannot converge.
    #[test]
    fn nested_yaws_receive_parent_local_targets() {
        let mut rig = AimRig::new();
        // Fixture: parent is already at 0.4 rad; the desired hull bearing is 0.9 rad.
        let parent_yaw = 0.4;
        rig.world.get_mut::<Transform>(rig.turret).unwrap().rotation =
            Quat::from_rotation_y(parent_yaw);
        let sight = rig
            .world
            .spawn((
                Transform::IDENTITY,
                ChildOf(rig.turret),
                ServoRole::Yaw,
                TankRoot(rig.tank),
                ServoRest(Quat::IDENTITY),
                ServoCommand::default(),
            ))
            .id();
        rig.world.entity_mut(rig.gun).insert(ChildOf(sight));
        let hull_origin = rig.pose_of(rig.hull, rig.tank).0;
        let desired = 0.9_f32;
        let point = hull_origin + crate::sight::hull_local_dir(desired, -SUPERELEVATION) * 1000.0;
        rig.world.get_mut::<TankCommand>(rig.tank).unwrap().aim = Some(AimIntent::World(point));
        rig.run(drive_aim_servos);
        assert!(
            (rig.world.get::<ServoCommand>(sight).unwrap().target - (desired - parent_yaw)).abs()
                < 1e-5
        );
    }

    /// A hand mount whose rate exceeds the hull pivot follows a world place
    /// without steady lag. A transport which rotates the intention in flight instead introduces
    /// ±ω*delivery delay, so both pivot directions must pass.
    #[test]
    fn a_hand_mount_follows_a_world_place_through_either_pivot() {
        /// Hull yaw rate, degrees/second — a brisk neutral-steer pivot.
        const OMEGA: f32 = 10.0;
        let target = ROOT + Vec3::new(0.0, 0.0, -500.0);

        let transport = OMEGA * DELIVERY_TICKS as f32 * TICK;

        for omega in [OMEGA, -OMEGA] {
            let mut rig = AimRig::new();
            rig.watch(target);
            rig.frame(0.0, 0.0, Author::ThirdPerson);
            let place = rig.picked();
            for t in 1..512 {
                let yaw = omega * t as f32 * TICK;
                rig.frame(yaw, yaw, Author::ThirdPerson);
            }
            let hull = rig.hull_attitude();
            let residual = rig.bore_miss_deg(place, hull).0.abs();
            assert!(
                residual < 0.05,
                "at ω = {omega}°/s, within the fixture hand rate, residual {residual:.3}° must stay near zero; rotating intention in flight biases it by ±ω·delivery = {transport:.3}°",
            );
        }
    }

    /// Free-look picks nothing: ADR-0001's unstabilized hold must sweep world bearing with the
    /// hull while holding hull-relative bearing fixed. A stored world point would counter-turn
    /// the gun to stabilize on its starting spot, failing both halves.
    #[test]
    fn a_held_bearing_sweeps_the_gun_with_the_hull() {
        const OMEGA: f32 = 10.0;
        /// Ticks skipped before sampling, so every sample is of a fully delivered intention.
        const SETTLE: u32 = 64;
        const RUN: u32 = 512;
        let target = ROOT + Vec3::new(0.0, 0.0, -500.0);

        let mut rig = AimRig::new();
        rig.watch(target);
        rig.frame(0.0, 0.0, Author::ThirdPerson);
        rig.right_mouse(true);

        let (mut world_span, mut hull_span) = ((f32::MAX, f32::MIN), (f32::MAX, f32::MIN));
        for t in 1..RUN {
            let yaw = OMEGA * t as f32 * TICK;
            rig.frame(yaw, yaw, Author::ThirdPerson);
            if t >= SETTLE {
                let (world, local) = (rig.commanded_bearing_deg(), rig.hull_relative_bearing_deg());
                world_span = (world_span.0.min(world), world_span.1.max(world));
                hull_span = (hull_span.0.min(local), hull_span.1.max(local));
            }
        }

        let swept = world_span.1 - world_span.0;
        let hull_turned = OMEGA * (RUN - 1 - SETTLE) as f32 * TICK;
        assert!(
            (swept - hull_turned).abs() < 0.05,
            "a held bearing must sweep the gun with the hull: the commanded world bearing moved \
             {swept:.3}° while the hull turned {hull_turned:.3}°. Equal means the gun rode round; \
             zero means the aim held a spot on the world and the mount stabilized onto it",
        );
        assert!(
            hull_span.1 - hull_span.0 < 1e-3,
            "the bearing off the hull is what the player is holding, so nothing may move it — got \
             {:.4}° of drift over a {hull_turned:.3}° pivot",
            hull_span.1 - hull_span.0,
        );
    }

    /// A still-mouse optic hold must keep hull-relative commanded bearing fixed while rendered
    /// hull freezes/steps and authority hull turns smoothly. Camera, working yaw/pitch and gun
    /// are hull-anchored; a client-world-authority round trip would import their pose gap as
    /// lay noise (ADR-0038).
    #[test]
    fn the_optic_intent_crosses_the_delivery_gap_untouched() {
        const OMEGA: f32 = 10.0;
        const FREEZE_TICKS: u32 = 6;
        /// The bearing the gunner is holding, in the hull's frame.
        const HELD: Vec3 = Vec3::new(60.0, 0.0, -500.0);

        let mut rig = AimRig::new();
        let tank = rig.tank;
        rig.world.resource_mut::<CommittedAim>().set(tank, HELD);

        let mut span = (f32::MAX, f32::MIN);
        for t in 0..512u32 {
            let shown = (t / FREEZE_TICKS * FREEZE_TICKS) as f32;
            rig.frame(OMEGA * shown * TICK, OMEGA * t as f32 * TICK, Author::Optic);
            assert!(
                matches!(rig.authored, Some(AimIntent::HullLocal(_))),
                "the optic authors in the frame it is anchored to",
            );
            if t >= 64 {
                let bearing = rig.hull_relative_bearing_deg();
                span = (span.0.min(bearing), span.1.max(bearing));
            }
        }

        let peak_to_peak = span.1 - span.0;
        let hull_step = OMEGA * FREEZE_TICKS as f32 * TICK;
        assert!(
            peak_to_peak < 1e-4,
            "a hull-anchored intention must reach the authority exactly as authored — got \
             {peak_to_peak:.4}° of swing in the commanded bearing off the hull, which a round trip \
             through the world imports from the {hull_step:.3}° gap between the client's rendered \
             hull and the authority's",
        );
    }

    /// Cover the optic's steering/publish arm as well as its hold arm: same mouse and delivery
    /// gap, smooth versus freeze/step rendered hull, must agree within the angular tolerance.
    /// The empty world leaves ray direction only choosing a sky miss; a world round trip would
    /// import the rendered hull's step into the lay (ADR-0038).
    #[test]
    fn steering_the_sight_never_lets_the_rendered_hull_in() {
        const OMEGA: f32 = 10.0;
        const FREEZE_TICKS: u32 = 6;
        /// A steady mouse drag, in raw counts per frame — off-axis, so both servos move.
        const STEER: Vec2 = Vec2::new(3.0, -1.0);
        const RUN: u32 = 256;

        let trace = |staircase: bool| {
            let mut rig = AimRig::new();
            let mut bearings = Vec::with_capacity(RUN as usize);
            for t in 0..RUN {
                rig.mouse_motion(STEER);
                let shown = if staircase {
                    (t / FREEZE_TICKS * FREEZE_TICKS) as f32
                } else {
                    t as f32
                };
                rig.frame(OMEGA * shown * TICK, OMEGA * t as f32 * TICK, Author::Optic);
                assert!(
                    matches!(rig.authored, Some(AimIntent::HullLocal(_))),
                    "a hull-anchored view authors in the frame it is anchored to, steering or not",
                );
                bearings.push(rig.hull_relative_bearing_deg());
            }
            bearings
        };

        let (smooth, stepped) = (trace(false), trace(true));
        let worst = smooth
            .iter()
            .zip(&stepped)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        let hull_step = OMEGA * FREEZE_TICKS as f32 * TICK;
        assert!(
            worst < 1e-4,
            "the optic let the client's rendered hull into the lay: the same steering commanded \
             bearings {worst:.4}° apart depending only on whether that hull staircased, against \
             the {hull_step:.3}° step it staircases by",
        );
    }

    /// Pin memory VALUE: motion-span tests pass even for a constant bearing CLAIMED hundreds of
    /// metres wrong (`transform_vector3` dropping translation, or a world point stored locally).
    /// Holding at rest must re-author the picked place; pivoting must sweep it hull-rigidly
    /// (ADR-0001). `sight::resume_commit` consumes this same value; wrong storage jumps on toggle.
    #[test]
    fn a_held_intention_still_names_the_place_it_was_picked_at() {
        const TURN_DEG: f32 = 20.0;
        let target = ROOT + Vec3::new(0.0, 0.0, -500.0);
        /// Where the hull's origin stands at a given heading — the point the held bearing is
        /// measured FROM, which is not the physics root and not the world origin.
        fn hull_origin(yaw_deg: f32) -> Vec3 {
            ROOT + Quat::from_rotation_y(yaw_deg.to_radians()) * HULL_OFFSET
        }

        let mut rig = AimRig::new();
        rig.watch(target);
        rig.frame(0.0, 0.0, Author::ThirdPerson);
        let place = rig.picked();

        rig.right_mouse(true);
        rig.frame(0.0, 0.0, Author::ThirdPerson);
        let slip = rig.picked().distance(place);
        assert!(
            slip < 1e-2,
            "letting go of the pick may not move the intention: the hold re-authored {slip:.3} m \
             from the place it was committed at, so the memory took it through the wrong transform",
        );

        rig.frame(TURN_DEG, TURN_DEG, Author::ThirdPerson);
        let swept = rig.picked();
        let rigid = Quat::from_rotation_y(TURN_DEG.to_radians()) * (place - hull_origin(0.0))
            + hull_origin(TURN_DEG);
        let error = swept.distance(rigid);
        assert!(
            error < 1e-2,
            "a hold is a bearing off the hull, so a {TURN_DEG}° pivot must carry it rigidly round: \
             the re-authored place stands {error:.3} m off where that bearing points",
        );
    }

    /// Superelevation rotates in the pitch mount's hull-up plane. Level hulls cannot distinguish
    /// it from world-up; rolled/pitched hulls separate the answers by degrees, and a world-frame
    /// lob would require extra traverse.
    #[test]
    fn the_superelevation_lob_rides_the_hull_not_the_world() {
        /// The hull's roll — the whole reason the two frames have different answers. Tilting the
        /// plane of elevation by this projects the lob onto the world's vertical as `cos ROLL`.
        const ROLL: f32 = 0.45;
        let target = ROOT + Vec3::new(0.0, 0.0, -500.0);
        let attitude = Quat::from_euler(EulerRot::YXZ, 0.35, 0.0, ROLL);

        let mut rig = AimRig::new();
        for _ in 0..512 {
            rig.relay_with(AimIntent::World(target), attitude);
        }

        let lobbed = SUPERELEVATION.to_degrees();
        let hull = rig.hull_attitude();
        let (azimuth, in_hull) = rig.bore_miss_deg(target, hull);
        assert!(
            azimuth.abs() < 0.02 && (in_hull - lobbed).abs() < 0.02,
            "in the hull's frame a converged bore owes zero bearing error and exactly the \
             {lobbed:.3}° lob, got {azimuth:.3}° and {in_hull:.3}°",
        );

        // The tilt's own prediction, halved: the sighting geometry perturbs it, so hold the frames
        // to separating by the order the roll demands rather than to a fitted number.
        let separation = lobbed * (1.0 - ROLL.cos());
        let (_, in_world) = rig.bore_miss_deg(target, Quat::IDENTITY);
        assert!(
            lobbed - in_world > separation * 0.5,
            "the two frames must actually disagree on a hull rolled {ROLL} rad, or the law above \
             proves nothing: the world frame reads {in_world:.3}° against the hull frame's \
             {in_hull:.3}°, a separation of {separation:.3}° short",
        );
    }

    /// Entity-keyed memory prevents respawn/SP Tab possession from replaying stale aim on a
    /// new tank; empty or mismatched keys read no commitment.
    #[test]
    fn committed_aim_is_entity_keyed() {
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        assert_ne!(a, b);

        let mut committed = CommittedAim::default();
        assert_eq!(committed.get(a), None, "empty memory has no commitment");

        let point = Vec3::new(1.0, 2.0, -3.0);
        committed.set(a, point);
        assert_eq!(
            committed.get(a),
            Some(point),
            "the keyed tank reads its aim"
        );
        assert_eq!(
            committed.get(b),
            None,
            "a different tank reads no commitment (stale intention never replays)"
        );

        let point_b = Vec3::new(-4.0, 0.0, 5.0);
        committed.set(b, point_b);
        assert_eq!(committed.get(b), Some(point_b));
        assert_eq!(
            committed.get(a),
            None,
            "the old tank's key is gone once rekeyed"
        );
    }
}
