//! Collider attachment refusal. Avian's `ColliderOf` insert hook returns without listing the
//! collider in its body's `RigidBodyColliders` when either side has no `GlobalTransform`, and
//! every child placement boundary (`child_poses`) reaches a moved body's colliders only through
//! that list: such a child stays at its birth pose. Avian's `PhysicsTransformPlugin` requires
//! `Transform` through `Position`; the networked composition disables it, so there a body spawned
//! without `Transform` took that path silently. The attachment is refused instead, naming both
//! entities and the fix.

use super::required_state::PendingOwners;
use avian3d::prelude::*;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;

/// Panics if either side lacks `GlobalTransform`; a not-yet-built owner is queued for First. Runs after
/// Avian's insert hook, on the same insertion, and reads both entities as that hook does: raw
/// component reads, which a disabled collider or body passes, as it passes the hook.
pub fn refuse_unposed_attachment(attached: On<Insert<ColliderOf>>, mut world: DeferredWorld) {
    let collider = attached.entity;
    let &ColliderOf { body } = world
        .get::<ColliderOf>(collider)
        .expect("an inserted ColliderOf is on its entity while its observers run");
    assert!(
        world.get::<GlobalTransform>(collider).is_some(),
        "collider {collider} attaches to body {body} without a GlobalTransform: spawn it with \
         Transform"
    );
    assert!(
        world.get::<GlobalTransform>(body).is_some(),
        "body {body} takes collider {collider} without a GlobalTransform: spawn it with Transform"
    );
    if world.get::<RigidBody>(body).is_none() {
        world
            .resource_mut::<PendingOwners>()
            .0
            .push((collider, body));
    }
}
