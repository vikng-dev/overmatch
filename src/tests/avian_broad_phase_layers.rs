//! Avian broad-phase invariant: empty layer masks do no pair-discovery work but retain their
//! spatial-query proxies. The server and the game's damage model raycasts rely on this split;
//! game state only. Fixture dimensions and poses below keep every box overlapping.

use avian3d::{collision::CollisionDiagnostics, prelude::*};
use bevy::{ecs::system::RunSystemOnce, prelude::*};

use super::avian_child_pose_churn::physics_app;

/// Exercise every tree, including static sensors, and both empty terms in `interacts_with`.
/// Removing the traversal guard wastes candidate visits; pruning tree membership hides ray hits;
/// caching eligibility prevents a collider with changed layers from making contacts again.
#[test]
fn pairless_layers_skip_discovery_but_keep_query_proxies() {
    for body in [
        Some(RigidBody::Static),
        Some(RigidBody::Kinematic),
        Some(RigidBody::Dynamic),
        None,
    ] {
        for layers in [
            CollisionLayers::new(0b10, LayerMask::NONE),
            CollisionLayers::ALL_FILTERS,
            CollisionLayers::NONE,
        ] {
            let mut app = physics_app();
            app.insert_resource(Gravity::ZERO);
            for body in [RigidBody::Static, RigidBody::Kinematic] {
                app.world_mut().spawn((
                    body,
                    Sensor,
                    Transform::default(),
                    Collider::cuboid(20.0, 20.0, 20.0),
                    CollisionLayers::new(0b01, LayerMask::ALL),
                ));
            }
            let mut collider = app.world_mut().spawn((
                Sensor,
                Transform::default(),
                Collider::cuboid(1.0, 1.0, 1.0),
                layers,
            ));
            if let Some(body) = body {
                collider.insert(body);
            }
            let collider = collider.id();
            // Prime the manual clock and let initial tree insertion finish.
            for _ in 0..4 {
                app.update();
            }
            app.world_mut()
                .get_mut::<Position>(collider)
                .expect("the fixture collider retains its pose")
                .0
                .x = 3.0;
            app.update();
            let candidates = app
                .world()
                .resource::<CollisionDiagnostics>()
                .broad_phase_candidates;
            assert_eq!(candidates, 0, "pairless layers must not traverse any tree");
            let graph = app.world().resource::<ContactGraph>();
            assert_eq!(graph.active_pairs().len(), 1, "only the two peers pair");
            assert!(graph.active_pairs()[0].is_touching());
            assert_eq!(graph.contact_pairs_with(collider).count(), 0);
            println!(
                "body={body:?} layers={layers:?}: candidates={candidates}, pairs=1, touching=1"
            );
            let hit = app
                .world_mut()
                .run_system_once(|query: SpatialQuery| {
                    query.cast_ray(
                        Vec3::new(3.0, 5.0, 0.0),
                        Dir3::NEG_Y,
                        10.0,
                        true,
                        &SpatialQueryFilter::from_mask(0b10),
                    )
                })
                .expect("the spatial query runs");
            if layers.memberships != LayerMask::NONE {
                assert_eq!(hit.map(|hit| hit.entity), Some(collider));
            } else {
                assert!(hit.is_none(), "query masks still respect memberships");
            }

            app.world_mut()
                .entity_mut(collider)
                .insert(CollisionLayers::new(0b10, LayerMask::ALL));
            app.world_mut()
                .get_mut::<Position>(collider)
                .expect("the fixture collider retains its pose")
                .0
                .x = -3.0;
            app.update();
            let graph = app.world().resource::<ContactGraph>();
            assert_eq!(graph.contact_pairs_with(collider).count(), 2);
            assert_eq!(
                graph
                    .active_pairs()
                    .iter()
                    .filter(|p| p.is_touching())
                    .count(),
                3,
                "restoring layers admits both overlapping peers"
            );
        }
    }
}
