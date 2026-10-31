//! GUARD for the vendored child-collider pose patches (`vendor/avian3d-0.7.0-child-pose` and
//! `vendor/lightyear_avian3d-0.30.1-child-pose`, `OVERMATCH_PATCH.md` in each; records:
//! `upstream/avian-child-collider-pose-churn.md`, `upstream/lightyear-avian-child-pass-visits.md`).
//!
//! Two invariants. First: a standing static body's child colliders keep their `Position`,
//! `Rotation` and `ColliderAabb` change ticks across physics steps and across lightyear_avian's
//! per-frame child pass, so Avian's `update_moved_collider_aabbs` skips them. Stock Avian 0.7.0 and
//! lightyear_avian3d 0.30.1 rewrite every child's pose unconditionally and fail the first test.
//! Second: lightyear_avian's child pass visits only the children whose offset, own pose,
//! attachment or body changed since it last ran, and still carries every one that did; stock
//! lightyear_avian3d 0.30.1 visits every child every frame and fails the third test. Who relies on
//! both: the dedicated server, whose map holds every building element and tree segment as a static
//! body's child collider, and whose ballistics reads a moving tank's armour, child colliders too,
//! at the pose this pass leaves between steps. A dependency bump that drops either patch fails here.
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
use lightyear::avian3d::plugin::LightyearAvianPlugin;

/// Avian's default plugins, one 64 Hz step per `update`: the manual clock advances 15 625 µs per
/// update, DERIVED as 1 s / 64.
pub(super) fn physics_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::transform::TransformPlugin,
        // Avian's collider cache reads `AssetEvent<Mesh>`.
        AssetPlugin::default(),
        PhysicsPlugins::default(),
    ))
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
/// shape of a building's element or a tree's segment. Returns `(root, child)` after the spawn has
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

/// A standing static body's child is never re-marked: not by Avian's step, not by lightyear_avian's
/// per-frame pass. Fails on stock Avian 0.7.0 or lightyear_avian3d 0.30.1 (see the module doc).
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
        "Avian's step re-marked an unmoved static child's (Position, Rotation, ColliderAabb): \
         is vendor/avian3d-0.7.0-child-pose still the avian3d in the graph?"
    );
    app.world_mut()
        .run_system_once(LightyearAvianPlugin::update_child_collider_position)
        .expect("the child pass runs");
    assert_eq!(
        ticks(app.world(), child),
        settled,
        "lightyear_avian's child pass re-marked an unmoved static child's pose: is \
         vendor/lightyear_avian3d-0.30.1-child-pose still the lightyear_avian3d in the graph?"
    );
}

/// The other half of the same patch: a static body that does move (a felled tree, a client's
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
        .run_system_once(LightyearAvianPlugin::update_child_collider_position)
        .expect("the child pass runs");
    assert_eq!(
        app.world().get::<Position>(child).expect("pose").0,
        expected(pose, turn),
        "lightyear_avian's child pass left the child behind its moved body"
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
        "Avian's step left the child behind its moved body"
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

/// lightyear_avian's child pass registered once and run once, so its change detection carries from
/// run to run as it does in `RunFixedMainLoop`. The first run visits every child.
fn registered_child_pass(app: &mut App) -> SystemId {
    let pass = app
        .world_mut()
        .register_system(LightyearAvianPlugin::update_child_collider_position);
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

/// The pose the child takes from the body it is attached to, by lightyear_avian's expressions.
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
        (turn.0 * local.rotation.0).normalize(),
    )
}

/// lightyear_avian's child pass reads no child whose offset, own pose, attachment and body all
/// stood since its last run: a pose planted there without a change tick survives the pass. Fails on
/// stock lightyear_avian3d 0.30.1, whose pass recomputes every child every frame.
#[test]
fn lightyears_child_pass_visits_no_child_whose_inputs_stood() {
    let mut app = physics_app();
    let (_, child) = standing_compound(&mut app);
    let pass = registered_child_pass(&mut app);
    let stale = plant_stale_pose(app.world_mut(), child);
    run(&mut app, pass);
    assert_eq!(
        pose(app.world(), child),
        stale,
        "lightyear_avian's child pass visited a child whose inputs all stood: is \
         vendor/lightyear_avian3d-0.30.1-child-pose still the lightyear_avian3d in the graph?"
    );
}

/// The other half: a change to any input the pass reads brings the child to its body again, over a
/// pose planted without a change tick.
#[test]
fn lightyears_child_pass_carries_a_child_through_each_input_change() {
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
            "lightyear_avian's child pass did not carry the child after {change}"
        );
    }
}

/// A child disabled while its body moved is carried once it is enabled again. Its re-enabling moves
/// none of the change ticks the pass reads, so the pass visits every child after a disabling
/// component's removal.
#[test]
fn lightyears_child_pass_carries_a_child_re_enabled_after_its_body_moved() {
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
        "lightyear_avian's child pass left a re-enabled child where its body stood"
    );
}
