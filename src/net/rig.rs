//! Local simulation attachment for replicated tank roots.
//!
//! ADR-0014 exception: a replicated root receives simulation only after it has a wire pose. The
//! adapter blocks rollback until physics prepares that body; children derive from root simulation.

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::prelude::*;
use lightyear::frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use super::protocol::NetTank;
use crate::tank::{PendingTankAssets, Rig, TankSimSource, attach_replicated_tank_body};

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(upgrade_predicted_to_dynamic);
    app.add_systems(
        FixedLast,
        enable_rollback_after_first_tick.run_if(not(is_in_rollback)),
    );
}

/// Attach simulation from `TankSimSource` only after a replicated root has a valid pose.
pub(crate) fn attach_replicated_rig(
    // Avoid registering Avian placeholder poses in rollback history.
    tanks: Query<
        (Entity, Has<Predicted>),
        (
            With<Remote>,
            With<NetTank>,
            With<Position>,
            With<Rotation>,
            Without<RigidBody>,
        ),
    >,
    assets: Option<Res<PendingTankAssets>>,
    source: TankSimSource,
    mut commands: Commands,
) {
    if tanks.is_empty() {
        return;
    }
    let Some(assets) = assets else { return };
    let Some(content) = source.get() else {
        return;
    };
    for (entity, predicted) in &tanks {
        let body = if predicted {
            RigidBody::Dynamic
        } else {
            RigidBody::Static
        };
        info!("client: {entity} replicated tank gets local sim body (predicted={predicted})");
        attach_replicated_tank_body(
            &mut commands,
            entity,
            content,
            assets.presentation(),
            (
                NetTank,
                // The local hierarchy requires `Transform`; Avian writes it from the wire pose.
                Transform::default(),
                body,
                // Block replay until one physics preparation pass has built collider transforms.
                DisableRollback,
            ),
        );
    }
}

/// Enable rollback after one complete physics tick has prepared a newly attached or promoted body.
fn enable_rollback_after_first_tick(
    fresh: Query<Entity, (With<Rig>, With<NetTank>, With<DisableRollback>)>,
    mut commands: Commands,
) {
    for entity in &fresh {
        info!("net: {entity} first physics tick complete — rollback enabled");
        commands.entity(entity).remove::<DisableRollback>();
    }
}

/// Promote a body when the independently replicated `Predicted` marker arrives late.
fn upgrade_predicted_to_dynamic(
    add: On<Add, Predicted>,
    eligible: Query<(), (With<Remote>, With<NetTank>, With<Rig>)>,
    mut commands: Commands,
) {
    if !eligible.contains(add.entity) {
        return;
    }
    info!(
        "net: {} predicted marker arrived after spawn — body goes Dynamic",
        add.entity
    );
    commands
        .entity(add.entity)
        .insert((RigidBody::Dynamic, DisableRollback));
}

/// Install Lightyear frame interpolation for predicted root position and rotation.
pub fn client_smoothing_plugin(app: &mut App) {
    app.add_plugins((
        FrameInterpolationPlugin::<Position>::default(),
        FrameInterpolationPlugin::<Rotation>::default(),
    ));
    app.add_systems(Update, arm_predicted_smoothing);
}

/// Arm only the root after independently replicated markers arrive; child poses are derived state.
fn arm_predicted_smoothing(
    tanks: Query<
        Entity,
        (
            With<Predicted>,
            With<NetTank>,
            With<Position>,
            Without<FrameInterpolate<Position>>,
        ),
    >,
    mut commands: Commands,
) {
    for entity in &tanks {
        info!("net: {entity} predicted root armed for frame interpolation + correction");
        commands.entity(entity).insert((
            FrameInterpolate::<Position>::default(),
            FrameInterpolate::<Rotation>::default(),
        ));
    }
}
