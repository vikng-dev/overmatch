//! GUARD for the vendored standalone-key write-back (`vendor/avian3d-0.7.0-child-pose`,
//! `OVERMATCH_PATCH.md`; record: `upstream/avian-collider-of-removal-orphan-proxy.md`).
//!
//! The invariant: every proxy in Avian's collider trees names a live collider, so a collider that
//! leaves its body and is later despawned takes its proxy with it, and a map despawned and rebuilt
//! in place holds the contact pairs of a fresh map. Who relies on it: the dedicated server, which
//! despawns and rebuilds the whole map between Battles and despawns tank parts through a combat;
//! an orphan proxy pairs with every collider its bounds overlap for the rest of the process, and
//! the narrow phase walks every pair each step. Stock Avian 0.7.0 moves a collider whose
//! `ColliderOf` is removed to the standalone tree without writing the new key back, and fails the
//! first test.
//!
//! The poses, box dimensions and update counts below are fixture data.

use avian3d::collider_tree::ColliderTrees;
use avian3d::prelude::*;
use bevy::prelude::*;

use super::avian_child_pose_churn::{physics_app, physics_app_on, standing_compound};

/// The colliders the four collider trees' proxies name, live or not.
fn proxy_colliders(world: &World) -> Vec<Entity> {
    let trees = world.resource::<ColliderTrees>();
    [
        &trees.dynamic_tree,
        &trees.kinematic_tree,
        &trees.static_tree,
        &trees.standalone_tree,
    ]
    .into_iter()
    .flat_map(|tree| tree.proxies.iter().map(|(_, proxy)| proxy.collider))
    .collect()
}

fn contact_pairs(world: &World) -> usize {
    let graph = world.resource::<ContactGraph>();
    graph.active_pairs().len() + graph.sleeping_pairs().len()
}

/// A collider whose body is removed becomes standalone; despawned later, it leaves no proxy.
/// Mutant: the standalone key not written back leaves the proxy, naming a dead entity.
#[test]
fn a_collider_that_left_its_body_takes_its_proxy_when_despawned() {
    let mut app = physics_app();
    let (root, child) = standing_compound(&mut app);
    app.world_mut().entity_mut(root).remove::<RigidBody>();
    app.update();
    assert!(
        app.world().get::<ColliderOf>(child).is_none(),
        "the fixture's collider still has a body"
    );
    assert_eq!(
        proxy_colliders(app.world()),
        vec![child],
        "the standalone collider is not in the trees once"
    );

    app.world_mut().entity_mut(child).despawn();
    app.update();
    assert_eq!(
        proxy_colliders(app.world()),
        Vec::<Entity>::new(),
        "a despawned collider's proxy outlived it"
    );
}

/// A static compound despawned and rebuilt in place, as the map is between Battles, leaves one
/// proxy per live collider and no contact pair: static colliders never pair with each other.
///
/// Bevy runs one despawn's `Remove` observers in component-id order. `ColliderOf` is registered
/// before Avian registers `Collider`, so Avian's `Remove<ColliderOf>` observer runs before its
/// `Remove<Collider>` one: the order in which the server's map rebuild leaked under the mutant
/// below (`.agents/scratch/perf-2026-11-01/server-tick.md`). Mutant: the standalone key not
/// written back leaves the old collider's proxy, which pairs with its successor.
#[test]
fn a_static_compound_rebuilt_in_place_holds_a_fresh_compounds_pairs() {
    let mut first = App::new();
    first.world_mut().register_component::<ColliderOf>();
    let mut app = physics_app_on(first);
    let (root, _) = standing_compound(&mut app);
    app.world_mut().entity_mut(root).despawn();
    app.update();
    let (_, child) = standing_compound(&mut app);
    assert_eq!(
        proxy_colliders(app.world()),
        vec![child],
        "the rebuilt compound's trees hold a proxy but its collider's"
    );
    assert_eq!(
        contact_pairs(app.world()),
        0,
        "the rebuilt static compound pairs with what its predecessor left"
    );
}
