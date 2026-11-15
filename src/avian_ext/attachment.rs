//! Collider attachment refusal. Avian's `ColliderOf` insert hook returns without listing the
//! collider in its body's `RigidBodyColliders` when either side has no `GlobalTransform`, and
//! every child placement boundary (`child_poses`) reaches a moved body's colliders only through
//! that list: such a child stays at its birth pose. Avian's `PhysicsTransformPlugin` requires
//! `Transform` through `Position`; the networked composition disables it, so there a body spawned
//! without `Transform` took that path silently. The attachment is refused instead, naming both
//! entities and the fix.

use avian3d::prelude::*;
use bevy::prelude::*;

/// Panics when a collider attaches to a body and either has no `GlobalTransform`. Runs after
/// Avian's insert hook, on the same insertion.
pub fn refuse_unposed_attachment(
    attached: On<Insert<ColliderOf>>,
    colliders: Query<(&ColliderOf, Has<GlobalTransform>)>,
    posed: Query<(), With<GlobalTransform>>,
) {
    let collider = attached.entity;
    let (&ColliderOf { body }, collider_posed) = colliders
        .get(collider)
        .expect("an inserted ColliderOf is on its entity while its observers run");
    assert!(
        collider_posed,
        "collider {collider} attaches to body {body} without a GlobalTransform: spawn it with \
         Transform"
    );
    assert!(
        posed.contains(body),
        "body {body} takes collider {collider} without a GlobalTransform: spawn it with Transform"
    );
}
