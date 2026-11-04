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

/// The game's damage model: an assembly leaving the tank loses its aim controllers throughout
/// its subtree. Its held local poses remain valid rig data, including when a view arrives later.
pub(super) fn stop_detached_assembly_servos(
    add: On<Add, crate::damage::LaunchedTurret>,
    servos: Query<
        (Entity, &Transform, Option<&super::view::ViewNode>),
        (
            With<super::servo::ServoSpec>,
            With<super::servo::ServoCommand>,
        ),
    >,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    for (servo, pose, view) in &servos {
        if servo == add.entity || parents.iter_ancestors(servo).any(|node| node == add.entity) {
            commands
                .entity(servo)
                .remove::<super::servo::ServoCommand>();
            if let Some(view) = view {
                commands
                    .entity(view.0)
                    .remove::<super::view::ViewServo>()
                    .insert(*pose);
            }
        }
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
pub(crate) fn authored_attachment(transform: Transform) -> impl Bundle {
    (transform, AuthoredLocalTransform(transform))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tank::view::{ViewNode, ViewServo};
    use crate::tank::{ServoCommand, ServoRole, ServoSpec};

    /// A detached two-part assembly must not retain commands which need a chain to its old tank.
    /// The shipped installer must also freeze bound views; unrelated mounts keep their commands.
    #[test]
    fn a_detached_assembly_stops_every_descendant_controller() {
        let mut app = App::new();
        app.add_plugins(crate::tank::sim_plugin);
        let world = app.world_mut();
        let tank = world.spawn_empty().id();
        let assembly = world.spawn(ChildOf(tank)).id();
        let pose = Transform::from_rotation(Quat::from_rotation_y(0.4)); // Fixture held bearing.
        let view = world.spawn((ViewServo, Transform::IDENTITY)).id();
        let yaw = world
            .spawn((
                ChildOf(assembly),
                pose,
                ViewNode(view),
                ServoCommand::default(),
                ServoSpec::test_continuous(ServoRole::Yaw, 1.0),
            ))
            .id();
        let pitch = world
            .spawn((
                ChildOf(yaw),
                Transform::IDENTITY,
                ServoCommand::default(),
                ServoSpec::test_continuous(ServoRole::Pitch, 1.0),
            ))
            .id();
        let other = world
            .spawn((
                ChildOf(tank),
                Transform::IDENTITY,
                ServoCommand::default(),
                ServoSpec::test_continuous(ServoRole::Yaw, 1.0),
            ))
            .id();
        world
            .entity_mut(assembly)
            .remove::<ChildOf>()
            .insert(crate::damage::LaunchedTurret);
        world.flush();
        for node in [yaw, pitch] {
            assert!(world.get::<ServoCommand>(node).is_none());
            assert!(
                world.get::<ServoSpec>(node).is_some(),
                "held construction remains available"
            );
        }
        assert!(world.get::<ServoCommand>(other).is_some());
        assert!(world.get::<ViewServo>(view).is_none());
        assert_eq!(world.get::<Transform>(view).unwrap(), &pose);
    }
}
