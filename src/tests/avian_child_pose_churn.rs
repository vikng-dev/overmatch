//! GUARD for the vendored child-collider pose patch (`vendor/avian3d-0.7.0-child-pose` and
//! `vendor/lightyear_avian3d-0.30.1-child-pose`, `OVERMATCH_PATCH.md` in each; record:
//! `upstream/avian-child-collider-pose-churn.md`).
//!
//! The invariant: a standing static body's child colliders keep their `Position`, `Rotation` and
//! `ColliderAabb` change ticks across physics steps and across lightyear_avian's per-frame child
//! pass, so Avian's `update_moved_collider_aabbs` skips them. Who relies on it: the dedicated
//! server's physics step, whose map holds every building element and tree segment as a static
//! body's child collider (its measured cost is in the record). Stock Avian 0.7.0 and
//! lightyear_avian3d 0.30.1 rewrite every child's pose unconditionally and fail the first test; a
//! dependency bump that drops either patch fails it too.
//!
//! The poses, box dimensions and update counts below are fixture data: any off-origin, turned pose
//! and any count of settling and observed updates exercise the same invariant.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::change_detection::Tick;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use lightyear::avian3d::plugin::LightyearAvianPlugin;

/// Avian's default plugins, one 64 Hz step per `update`: the manual clock advances 15 625 µs per
/// update, DERIVED as 1 s / 64.
fn physics_app() -> App {
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
fn standing_compound(app: &mut App) -> (Entity, Entity) {
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
