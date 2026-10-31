//! A collider moved onto another rigid body drops its contact pairs, so the physics makes them again
//! under the body it belongs to now. Game state only.
//!
//! THE INVARIANT: every contact pair names the bodies its two colliders belong to now. Avian 0.7
//! records a pair's bodies once, when its broad phase makes the pair, and never rewrites them when a
//! collider's [`ColliderOf`] changes; a stale pair corrupts Avian's island lists. The game rehomes
//! colliders in one place, `damage::launch_turrets_on_cookoff`.
//!
//! [`drop_stale_pairs`] runs whenever `ColliderOf` is inserted. When one of the collider's pairs
//! names another body for it, the collider is disabled and enabled again in one command batch, so
//! Avian removes the pairs and the broad phase remakes them under the new body. A re-insert naming
//! the same body is left alone. Mechanism, evidence and removal condition:
//! `upstream/avian-collider-rehome-stale-pair-bodies.md`.

use avian3d::prelude::{ColliderDisabled, ColliderOf, ContactGraph};
use bevy::prelude::*;

/// `On<Insert, ColliderOf>`: drop the collider's pairs when any names another body for it.
pub(crate) fn drop_stale_pairs(
    rehomed: On<Insert, ColliderOf>,
    colliders: Query<&ColliderOf>,
    graph: Option<Res<ContactGraph>>,
    mut commands: Commands,
) {
    let collider = rehomed.entity;
    let (Ok(&ColliderOf { body }), Some(graph)) = (colliders.get(collider), graph) else {
        return;
    };
    let stale = graph.contact_edges_with(collider).any(|edge| {
        let named = if edge.collider1 == collider {
            edge.body1
        } else {
            edge.body2
        };
        named != Some(body)
    });
    if stale {
        commands
            .entity(collider)
            .try_insert(ColliderDisabled)
            .try_remove::<ColliderDisabled>();
    }
}

/// Mount the rule wherever the simulation runs.
pub(crate) fn plugin(app: &mut App) {
    app.add_observer(drop_stale_pairs);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use avian3d::prelude::*;
    use bevy::prelude::*;
    use bevy::time::TimeUpdateStrategy;

    /// Avian's default plugins, islands included, one 64 Hz step per `update` (15 625 µs, DERIVED
    /// as 1 s / 64), with or without the rule.
    fn physics_app(rule: bool) -> App {
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
        if rule {
            app.add_plugins(super::plugin);
        }
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app
    }

    /// A dynamic hull resting on static ground, with a child node beside it whose box also rests on
    /// the ground, settled; then a second dynamic body at the node's world pose, and the box moved
    /// onto it. Returns `(hull, turret, turret_box, ground)`. Sizes and offsets are fixture data:
    /// any collider moved onto another body while it touches a third exercises the same invariant.
    ///
    /// The box moves onto a body that already exists, not onto the node made a body in place as a
    /// launch does: that path can hide the defect, depending on Avian's observer order (see the
    /// upstream draft).
    fn rehomed(app: &mut App) -> (Entity, Entity, Entity, Entity) {
        let world = app.world_mut();
        let ground = world
            .spawn((
                RigidBody::Static,
                Position(Vec3::new(0.0, -0.5, 0.0)),
                Collider::cuboid(40.0, 1.0, 40.0),
            ))
            .id();
        let hull = world
            .spawn((
                RigidBody::Dynamic,
                Transform::from_xyz(0.0, 0.5, 0.0),
                Position(Vec3::new(0.0, 0.5, 0.0)),
            ))
            .id();
        world.spawn((
            ChildOf(hull),
            Transform::IDENTITY,
            Collider::cuboid(2.0, 1.0, 2.0),
        ));
        let node = world
            .spawn((ChildOf(hull), Transform::from_xyz(3.0, 0.0, 0.0)))
            .id();
        let turret_box = world
            .spawn((
                ChildOf(node),
                Transform::IDENTITY,
                Collider::cuboid(1.0, 1.0, 1.0),
            ))
            .id();
        for _ in 0..8 {
            app.update();
        }
        let (edge, pair) = app
            .world()
            .resource::<ContactGraph>()
            .get(turret_box, ground)
            .expect("fixture: the box rests on the ground");
        assert!(edge.is_touching(), "fixture: the pair touches");
        let bodies = [pair.body1, pair.body2];
        assert!(
            bodies.contains(&Some(hull)) && bodies.contains(&Some(ground)),
            "fixture: before the move the pair names the hull and the ground, not {bodies:?}"
        );
        let pose = app
            .world()
            .get::<GlobalTransform>(node)
            .expect("the node is placed")
            .compute_transform();
        let turret = app
            .world_mut()
            .spawn((
                Position(pose.translation),
                Rotation(pose.rotation),
                pose,
                RigidBody::Dynamic,
            ))
            .id();
        app.world_mut()
            .entity_mut(turret_box)
            .insert(ChildOf(turret));
        app.update();
        app.update();
        (hull, turret, turret_box, ground)
    }

    /// The bodies `collider`'s pairs name for it.
    fn named_bodies(world: &World, collider: Entity) -> Vec<Option<Entity>> {
        world
            .resource::<ContactGraph>()
            .contact_edges_with(collider)
            .map(|edge| {
                if edge.collider1 == collider {
                    edge.body1
                } else {
                    edge.body2
                }
            })
            .collect()
    }

    /// THE INVARIANT (module doc): once a turret's collider belongs to the turret, every pair it
    /// holds names the turret, it still rests on the ground, and the hull's despawn and then the
    /// turret's leave Avian's islands whole. Without the rule the pair keeps naming the hull: the
    /// hull's island is removed with that pair still linked (a debug build stops on Avian's
    /// `contact_count == 0` assertion there) and the turret's despawn then unlinks it from an
    /// island that no longer exists, the dedicated server's panic.
    #[test]
    fn a_rehomed_colliders_pairs_name_its_new_body() {
        let mut app = physics_app(true);
        let (hull, turret, turret_box, ground) = rehomed(&mut app);
        let named = named_bodies(app.world(), turret_box);
        assert!(!named.is_empty(), "the turret's box holds its pairs again");
        assert!(
            named.iter().all(|body| *body == Some(turret)),
            "a pair names {named:?} for the turret's box, which belongs to {turret}"
        );
        assert!(
            app.world()
                .resource::<ContactGraph>()
                .get(turret_box, ground)
                .is_some_and(|(edge, _)| edge.is_touching()),
            "the turret's box still rests on the ground"
        );
        app.world_mut().entity_mut(hull).despawn();
        app.update();
        app.world_mut().entity_mut(turret).despawn();
        app.update();
        assert_eq!(
            app.world()
                .resource::<avian3d::dynamics::solver::islands::PhysicsIslands>()
                .iter()
                .map(|island| island.contact_count())
                .sum::<u32>(),
            0,
            "no island holds a contact once both bodies are gone"
        );
    }

    /// UPSTREAM TRIPWIRE, inverted: it passes while Avian 0.7 keeps a pair's bodies from the
    /// pair's birth. When it fails, Avian rewrites them on a `ColliderOf` change, and the rule
    /// ([`super::drop_stale_pairs`]) can go with its record,
    /// `upstream/avian-collider-rehome-stale-pair-bodies.md`.
    #[test]
    fn avian_still_names_a_rehomed_colliders_former_body() {
        let mut app = physics_app(false);
        let (hull, _, turret_box, _) = rehomed(&mut app);
        assert!(
            named_bodies(app.world(), turret_box).contains(&Some(hull)),
            "the rehomed collider's pair no longer names the former body: retire the rule (see this test's doc)"
        );
    }
}
