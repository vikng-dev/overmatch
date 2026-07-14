use avian3d::physics_transform::ApplyPosToTransform;
use avian3d::prelude::RigidBody;
use bevy::prelude::*;

use super::model::Rig;

/// Sweep a cooked-off turret when its root goes away; launched turrets are no longer descendants.
pub(super) fn sweep_launched_turret_on_root_despawn(
    remove: On<Remove, Rig>,
    rigs: Query<&Rig>,
    mut commands: Commands,
) {
    if let Ok(rig) = rigs.get(remove.entity) {
        commands.entity(rig.turret).try_despawn();
    }
}

/// Exact local pose for an authored child collider. ADR-0015 requires the observers below because
/// Lightyear-Avian can add [`ApplyPosToTransform`] to non-body children and feed render propagation
/// back into simulation. Remove this shield when upstream excludes such children or exposes an
/// entity-level sync opt-out.
#[derive(Component)]
pub struct AuthoredLocalTransform(pub Transform);

/// Remove a newly armed transform writer and restore the authored pose in the same command flush.
pub(super) fn shield_authored_collider_transform(
    add: On<Add, ApplyPosToTransform>,
    authored: Query<&AuthoredLocalTransform, Without<RigidBody>>,
    mut commands: Commands,
) {
    if let Ok(authored) = authored.get(add.entity) {
        // `try_*` tolerates a same-flush recursive despawn.
        commands
            .entity(add.entity)
            .try_remove::<ApplyPosToTransform>()
            .try_insert(authored.0);
    }
}

/// Mirror the shield when the authored marker is inserted after the transform writer.
pub(super) fn shield_late_authored_marker(
    add: On<Add, AuthoredLocalTransform>,
    armed: Query<&AuthoredLocalTransform, (With<ApplyPosToTransform>, Without<RigidBody>)>,
    mut commands: Commands,
) {
    if let Ok(authored) = armed.get(add.entity) {
        commands
            .entity(add.entity)
            .try_remove::<ApplyPosToTransform>()
            .try_insert(authored.0);
    }
}

/// Construct the live and recorded poses from one value so the shield cannot restore stale data.
pub(super) fn authored_attachment(transform: Transform) -> impl Bundle {
    (transform, AuthoredLocalTransform(transform))
}
