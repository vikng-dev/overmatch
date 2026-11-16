//! GUARD for child collider placement, the game's half of the vendored avian3d fork
//! (`crate::avian_ext::child_poses`; fork: `vendor/avian3d-0.8.0-overmatch`, `OVERMATCH_PATCH.md`,
//! "Child collider poses"; record: `upstream/avian-child-collider-pose-churn.md`).
//!
//! Two invariants. First: a standing static body's child colliders keep their `Position`,
//! `Rotation` and `ColliderAabb` change ticks across physics steps and across the game's
//! frame-boundary pass, so Avian's `update_moved_collider_aabbs` skips them. Stock Avian 0.8.0
//! rewrites every child's pose every step. Second: the placement pass visits only the children
//! whose offset, own pose, attachment or body changed since it last ran, and still carries every
//! one that did. Who relies on both: the dedicated server, whose map holds every intact
//! construction element as a static body's child collider, and whose ballistics reads a moving
//! tank's armour, child colliders too, at the pose this pass leaves between steps. A dependency
//! bump that registers Avian's own child pass again fails every physics test here (MEASURED on
//! stock 0.8.0): both passes write child poses in `PhysicsStepSystems::First`, and Avian's
//! `PhysicsSchedule` refuses that ambiguity when it initializes. One that registers
//! lightyear_avian's own pass at the frame boundary again fails
//! `lightyear_position_mode_leaves_the_frame_boundary_to_the_game` (MEASURED).
//!
//! The poses, box dimensions and update counts below are fixture data: any off-origin, turned pose
//! and any count of settling and observed updates exercise the same invariants.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::change_detection::Tick;
use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::system::{RunSystemOnce, SystemId};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use crate::avian_ext::child_poses::{ReenabledPlugin, place_child_colliders};

/// Avian's default plugins and the game's half of the fork, one 64 Hz step per `update`: the manual
/// clock advances 15 625 µs per update, DERIVED as 1 s / 64. Contact recycling is off, as the game
/// composes physics (`net::physics`): a recycled pair keeps last step's contacts and skips the
/// narrow phase's matching, cache and hooks.
pub(super) fn physics_app() -> App {
    physics_app_on(App::new())
}

/// [`physics_app`] built on `app`, for a test that registers something before Avian does.
pub(super) fn physics_app_on(mut app: App) -> App {
    app.add_plugins((
        MinimalPlugins,
        bevy::transform::TransformPlugin,
        // Avian's collider cache reads `AssetEvent<Mesh>`.
        AssetPlugin::default(),
        PhysicsPlugins::default(),
    ))
    .add_plugins(crate::avian_ext::plugin)
    .insert_resource(NarrowPhaseConfig {
        recycle_distance: 0.0,
        ..default()
    })
    .init_asset::<Mesh>()
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
        15_625,
    )));
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
    app
}

/// A static root, posed off the origin and turned, with one turned, offset box collider child: the
/// shape of an intact construction element. Returns `(root, child)` after the spawn has
/// settled.
pub(super) fn standing_compound(app: &mut App) -> (Entity, Entity) {
    let root = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Transform::from_xyz(3.0, 1.0, -2.0).with_rotation(Quat::from_rotation_y(0.7)),
        ))
        .id();
    let child = app
        .world_mut()
        .spawn((
            ChildOf(root),
            Transform::from_xyz(0.5, 1.0, 0.25).with_rotation(Quat::from_rotation_x(0.3)),
            Collider::cuboid(1.0, 2.0, 0.5),
        ))
        .id();
    for _ in 0..4 {
        app.update();
    }
    (root, child)
}

/// The change ticks of the child's pose and bounds.
fn ticks(world: &World, child: Entity) -> [Tick; 3] {
    let entity = world.entity(child);
    [
        entity.get_ref::<Position>().expect("pose").last_changed(),
        entity.get_ref::<Rotation>().expect("pose").last_changed(),
        entity
            .get_ref::<ColliderAabb>()
            .expect("bounds")
            .last_changed(),
    ]
}

/// A standing static body's child is never re-marked: not by the physics step, not by the frame
/// boundary's pass. Fails on stock Avian 0.8.0 (see the module doc).
#[test]
fn a_standing_static_bodys_child_collider_is_never_re_marked() {
    let mut app = physics_app();
    let (_, child) = standing_compound(&mut app);
    let settled = ticks(app.world(), child);
    for _ in 0..8 {
        app.update();
    }
    assert_eq!(
        ticks(app.world(), child),
        settled,
        "the physics step re-marked an unmoved static child's (Position, Rotation, ColliderAabb)"
    );
    app.world_mut()
        .run_system_once(place_child_colliders)
        .expect("the child pass runs");
    assert_eq!(
        ticks(app.world(), child),
        settled,
        "the frame boundary's child pass re-marked an unmoved static child's pose"
    );
}

/// The other half of the same patch: a static body that does move (a client's
/// launched turret) still carries its child, through both passes, and the child's bounds follow.
#[test]
fn a_moved_static_bodys_child_collider_follows_it() {
    let mut app = physics_app();
    let (root, child) = standing_compound(&mut app);
    let local = *app
        .world()
        .get::<ColliderTransform>(child)
        .expect("the child's offset");
    let expected = |pose: Position, turn: Rotation| pose.0 + turn * local.translation;

    let (pose, turn) = (
        Position(Vec3::new(-4.0, 2.0, 6.0)),
        Rotation(Quat::from_rotation_y(-0.4)),
    );
    app.world_mut().entity_mut(root).insert((pose, turn));
    app.world_mut()
        .run_system_once(place_child_colliders)
        .expect("the child pass runs");
    assert_eq!(
        app.world().get::<Position>(child).expect("pose").0,
        expected(pose, turn),
        "the child pass left the child behind its moved body"
    );

    let (pose, turn) = (
        Position(Vec3::new(5.0, 0.5, 1.0)),
        Rotation(Quat::from_rotation_y(1.1)),
    );
    let bounds = *app.world().get::<ColliderAabb>(child).expect("bounds");
    app.world_mut().entity_mut(root).insert((pose, turn));
    app.update();
    let world = app.world();
    assert_eq!(
        world.get::<Position>(child).expect("pose").0,
        expected(pose, turn),
        "the physics step left the child behind its moved body"
    );
    let moved = *world.get::<ColliderAabb>(child).expect("bounds");
    assert_ne!(
        moved, bounds,
        "the child's bounds stayed where the body stood"
    );
    let centre = world.get::<Position>(child).expect("pose").0;
    assert!(
        moved.min.cmple(centre).all() && centre.cmple(moved.max).all(),
        "the child's bounds do not hold its own centre"
    );
}

/// The child pass registered once and run once, so its change detection carries from run to run as
/// it does at the frame boundary. The first run visits every child.
fn registered_child_pass(app: &mut App) -> SystemId {
    let pass = app.world_mut().register_system(place_child_colliders);
    run(app, pass);
    pass
}

fn run(app: &mut App, pass: SystemId) {
    app.world_mut()
        .run_system(pass)
        .expect("the child pass runs");
}

/// A pose no body gives the child, written without a change tick: it survives a run of the pass
/// only if the pass does not visit the child.
fn plant_stale_pose(world: &mut World, child: Entity) -> (Vec3, Quat) {
    let stale = (Vec3::splat(100.0), Quat::from_rotation_z(1.0));
    let mut entity = world.entity_mut(child);
    entity
        .get_mut::<Position>()
        .expect("pose")
        .bypass_change_detection()
        .0 = stale.0;
    entity
        .get_mut::<Rotation>()
        .expect("pose")
        .bypass_change_detection()
        .0 = stale.1;
    stale
}

fn pose(world: &World, child: Entity) -> (Vec3, Quat) {
    (
        world.get::<Position>(child).expect("pose").0,
        world.get::<Rotation>(child).expect("pose").0,
    )
}

/// The pose the child takes from the body it is attached to.
fn carried_pose(world: &World, child: Entity) -> (Vec3, Quat) {
    let local = world
        .get::<ColliderTransform>(child)
        .expect("the child's offset");
    let body = world
        .get::<ColliderOf>(child)
        .expect("an attached child")
        .body;
    let pose = world.get::<Position>(body).expect("pose");
    let turn = world.get::<Rotation>(body).expect("pose");
    (
        pose.0 + turn * local.translation,
        (turn.0 * local.rotation).normalize(),
    )
}

/// The child pass reads no child whose offset, own pose, attachment and body all stood since its
/// last run: a pose planted there without a change tick survives the pass. Fails on a pass that
/// recomputes every child every run, as Avian's and lightyear_avian's own do.
#[test]
fn the_child_pass_visits_no_child_whose_inputs_stood() {
    let mut app = physics_app();
    let (_, child) = standing_compound(&mut app);
    let pass = registered_child_pass(&mut app);
    let stale = plant_stale_pose(app.world_mut(), child);
    run(&mut app, pass);
    assert_eq!(
        pose(app.world(), child),
        stale,
        "the child pass visited a child whose inputs all stood"
    );
}

/// The other half: a change to any input the pass reads brings the child to its body again, over a
/// pose planted without a change tick.
#[test]
fn the_child_pass_carries_a_child_through_each_input_change() {
    let mut app = physics_app();
    let (body, child) = standing_compound(&mut app);
    let (other_body, _) = standing_compound(&mut app);
    let pass = registered_child_pass(&mut app);
    let changes: [(&str, Box<dyn FnOnce(&mut World)>); 5] = [
        (
            "its body moved",
            Box::new(move |world| world.get_mut::<Position>(body).expect("pose").0 += Vec3::X),
        ),
        (
            "its body turned",
            Box::new(move |world| {
                world.get_mut::<Rotation>(body).expect("pose").0 *= Quat::from_rotation_x(0.2);
            }),
        ),
        (
            "its offset changed",
            Box::new(move |world| {
                world
                    .get_mut::<ColliderTransform>(child)
                    .expect("offset")
                    .translation += Vec3::Y;
            }),
        ),
        (
            "its own pose was written",
            Box::new(move |world| world.get_mut::<Position>(child).expect("pose").0 = Vec3::ZERO),
        ),
        (
            "it was attached to another body",
            Box::new(move |world| {
                world
                    .entity_mut(child)
                    .insert(ColliderOf { body: other_body });
            }),
        ),
    ];
    for (change, apply) in changes {
        plant_stale_pose(app.world_mut(), child);
        apply(app.world_mut());
        run(&mut app, pass);
        let world = app.world();
        assert_eq!(
            pose(world, child),
            carried_pose(world, child),
            "the child pass did not carry the child after {change}"
        );
    }
}

/// A child disabled while its body moved is carried once it is enabled again. Its re-enabling moves
/// none of the change ticks the pass reads, so the pass places each entity a `Reenabled` message
/// names.
#[test]
fn the_child_pass_carries_a_child_re_enabled_after_its_body_moved() {
    let mut app = physics_app();
    let (body, child) = standing_compound(&mut app);
    let pass = registered_child_pass(&mut app);
    app.world_mut().entity_mut(child).insert(Disabled);
    app.world_mut().get_mut::<Position>(body).expect("pose").0 += Vec3::X;
    run(&mut app, pass);
    app.world_mut().entity_mut(child).remove::<Disabled>();
    run(&mut app, pass);
    let world = app.world();
    assert_eq!(
        pose(world, child),
        carried_pose(world, child),
        "the child pass left a re-enabled child where its body stood"
    );
}

/// The physics step's placement pass carries a child re-enabled in `Update` frames before the next
/// fixed step, after its body moved. Render-driven clients can run several frames between ticks,
/// while Bevy clears removal messages every frame. The walk relies on the re-enable surviving until
/// the step so its collider pose follows the body. Fixture cadence: four frames per tick.
#[test]
fn the_physics_step_carries_a_child_re_enabled_frames_before_the_next_step() {
    #[derive(Resource, Default)]
    struct Steps(u32);
    #[derive(Resource, Default)]
    struct Enable(Option<Entity>);
    let mut app = physics_app();
    let (body, child) = standing_compound(&mut app);
    app.world_mut().entity_mut(child).insert(Disabled);
    app.world_mut().get_mut::<Position>(body).expect("pose").0 += Vec3::X;
    app.update();
    // DERIVED: a quarter of the 64 Hz step, 15 625 us / 4.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_nanos(
        3_906_250,
    )))
    .init_resource::<Steps>()
    .init_resource::<Enable>()
    .add_systems(FixedPostUpdate, |mut steps: ResMut<Steps>| steps.0 += 1)
    .add_systems(
        Update,
        |mut enable: ResMut<Enable>, mut commands: Commands| {
            if let Some(entity) = enable.0.take() {
                commands.entity(entity).remove::<Disabled>();
            }
        },
    );
    let stepped = |app: &mut App| {
        let before = app.world().resource::<Steps>().0;
        app.update();
        app.world().resource::<Steps>().0 != before
    };
    while !stepped(&mut app) {}
    app.world_mut().resource_mut::<Enable>().0 = Some(child);
    let mut frames_between = 0;
    while !stepped(&mut app) {
        frames_between += 1;
    }
    assert!(
        frames_between >= 2,
        "the removal must outlive Bevy's two-frame removal buffer: {frames_between} frames"
    );
    let world = app.world();
    assert_eq!(
        pose(world, child),
        carried_pose(world, child),
        "the physics step left a child re-enabled between steps where its body stood"
    );
}

/// Every placement boundary reaches a moved body's colliders only through `RigidBodyColliders`,
/// which Avian's `ColliderOf` hook fills only when both entities have a `GlobalTransform`. The
/// networked composition disables `PhysicsTransformPlugin`, which otherwise requires `Transform`
/// on every body, so there a body spawned without one is refused at the attachment; stock Avian
/// skips the link and leaves the child at its birth pose whenever the body moves.
#[test]
#[should_panic(expected = "without a GlobalTransform")]
fn a_collider_attached_to_a_body_without_a_global_transform_is_refused() {
    let mut app = networked_attachment_app();
    let world = app.world_mut();
    let body = world
        .spawn((RigidBody::Static, Position(Vec3::ZERO), Rotation::IDENTITY))
        .id();
    world.spawn((
        Collider::sphere(0.5),
        Position(Vec3::X),
        Rotation::IDENTITY,
        GlobalTransform::from_translation(Vec3::X),
        ColliderOf { body },
    ));
}

/// The other half: a posed attachment stands whichever side is disabled, as Avian's hook, whose
/// raw reads see a disabled entity, attaches it. Fails if the refusal reads either side through a
/// default-filtered query, which skips a `Disabled` entity and refuses the posed attachment.
#[test]
fn a_posed_attachment_stands_with_either_side_disabled() {
    for (body_disabled, collider_disabled) in [(true, false), (false, true)] {
        let mut app = networked_attachment_app();
        let world = app.world_mut();
        let mut body = world.spawn((
            RigidBody::Static,
            Position(Vec3::ZERO),
            Rotation::IDENTITY,
            GlobalTransform::IDENTITY,
        ));
        if body_disabled {
            body.insert(Disabled);
        }
        let body = body.id();
        let mut collider = world.spawn((
            Collider::sphere(0.5),
            Position(Vec3::X),
            Rotation::IDENTITY,
            GlobalTransform::from_translation(Vec3::X),
        ));
        if collider_disabled {
            collider.insert(Disabled);
        }
        let collider = collider.insert(ColliderOf { body }).id();
        assert!(
            world
                .get::<RigidBodyColliders>(body)
                .is_some_and(|colliders| colliders.contains(&collider)),
            "body disabled {body_disabled}, collider disabled {collider_disabled}: Avian's hook \
             left the posed collider off its body's list"
        );
    }
}

/// Avian's plugins as the networked composition mounts them (`PhysicsTransformPlugin` disabled),
/// and the game's seams.
fn networked_attachment_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        PhysicsPlugins::default()
            .build()
            .disable::<PhysicsTransformPlugin>(),
    ))
    .add_plugins(crate::avian_ext::plugin);
    app
}

/// ColliderOf is the actual physics attachment, including manually attached colliders without a
/// Transform child relationship. A moved body's collider list must carry that child too.
#[test]
fn a_manually_attached_child_follows_its_body_without_transform_children() {
    let mut app = App::new();
    app.add_plugins(ReenabledPlugin);
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Position(Vec3::ZERO),
            Rotation::IDENTITY,
            GlobalTransform::IDENTITY,
        ))
        .id();
    let child = app
        .world_mut()
        .spawn((
            Collider::sphere(0.5),
            Position(Vec3::ZERO),
            Rotation::IDENTITY,
            GlobalTransform::from_translation(Vec3::X),
            ColliderOf { body },
            ColliderTransform {
                translation: Vec3::X,
                ..default()
            },
        ))
        .id();
    let pass = app.world_mut().register_system(place_child_colliders);
    run(&mut app, pass);
    app.world_mut().get_mut::<Position>(body).unwrap().0 += Vec3::Z;
    run(&mut app, pass);
    assert_eq!(pose(app.world(), child).0, Vec3::X + Vec3::Z);
}

/// Lightyear's Position mode, composed as the game composes it (`net::protocol`, mounted beside
/// `net::physics`), registers no child-pose pass of its own in `RunFixedMainLoop`: the frame
/// boundary's slot holds the game's pass alone. Fails if the registration of
/// `LightyearAvianPlugin::update_child_collider_position` the vendored crate drops
/// (`vendor/lightyear_avian3d-0.31.0-child-pose`, `OVERMATCH_PATCH.md`) returns: that pass writes
/// every child's pose through `Mut` each frame, so no change tick survives it.
#[test]
fn lightyear_position_mode_leaves_the_frame_boundary_to_the_game() {
    use lightyear::avian3d::plugin::{AvianReplicationMode, LightyearAvianPlugin};
    let mut app = App::new();
    app.add_plugins(LightyearAvianPlugin {
        replication_mode: AvianReplicationMode::Position {
            sync_to_transform: false,
        },
        register_physics_components: false,
        ..default()
    });
    crate::net::physics::plugin(&mut app);
    let sets = &app
        .get_schedule(RunFixedMainLoop)
        .expect("the game's pass registers in RunFixedMainLoop")
        .graph()
        .system_sets;
    assert!(
        sets.contains(place_child_colliders.into_system_set()),
        "fixture: the game's pass holds the frame boundary"
    );
    assert!(
        !sets.contains(LightyearAvianPlugin::update_child_collider_position.into_system_set()),
        "lightyear registers its own child-pose pass at the frame boundary"
    );
}
