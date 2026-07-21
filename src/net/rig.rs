//! Local simulation attachment for replicated tank roots.
//!
//! ADR-0014 exception: a replicated root receives simulation only after it has a wire pose. The
//! adapter blocks rollback until physics prepares that body; children derive from root simulation.

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::prelude::*;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use super::protocol::NetTank;
use crate::tank::{PendingTankAssets, Rig, TankSimSource, WeaponGate, attach_replicated_tank_body};
use crate::track::sim::{TankTransmission, TrackGripElements};

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(queue_predicted_promotion)
        .add_systems(Update, upgrade_predicted_to_dynamic);
    app.add_systems(
        FixedLast,
        enable_rollback_after_first_tick.run_if(not(is_in_rollback)),
    );
}

/// A late `Predicted` marker arrived on an already attached interpolated rig. Keep the body static
/// until the same exact-field gate used by initial JIP attachment is satisfied.
#[derive(Component)]
struct PendingPredictedPromotion;

fn replica_role_ready(
    predicted: bool,
    interpolated: bool,
    grip_elements: Option<&TrackGripElements>,
    link_count: usize,
) -> bool {
    (predicted || interpolated)
        && (!predicted || grip_elements.is_some_and(|field| field.is_sized_for(link_count)))
}

#[cfg(test)]
pub(super) fn replica_role_ready_for_test(
    predicted: bool,
    interpolated: bool,
    grip_elements: Option<&TrackGripElements>,
    link_count: usize,
) -> bool {
    replica_role_ready(predicted, interpolated, grip_elements, link_count)
}

/// Attach simulation from `TankSimSource` only after a replicated root has a valid pose.
pub(crate) fn attach_replicated_rig(
    // Avoid registering Avian placeholder poses in rollback history.
    tanks: Query<
        (
            Entity,
            Has<Predicted>,
            Has<Interpolated>,
            Option<&TrackGripElements>,
            Option<&WeaponGate>,
        ),
        (
            With<Remote>,
            With<NetTank>,
            With<Position>,
            With<Rotation>,
            // The replicated current transmission snapshot must precede body attachment, so the
            // first predicted fixed tick cannot run on a freshly reconstructed JIP value.
            With<TankTransmission>,
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
    for (entity, predicted, interpolated, grip_elements, weapon_gate) in &tanks {
        // Wait until Lightyear declares the replica's role. An owner may not enter its first fixed
        // tick until the replicate-once exact field is present and sized; an interpolated remote
        // deliberately receives no private element state.
        if !replica_role_ready(
            predicted,
            interpolated,
            grip_elements,
            content.spec().track.link_count,
        ) || (predicted && weapon_gate.is_none())
        {
            continue;
        }
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
    fresh: Query<(Entity, Has<Predicted>), (With<Rig>, With<NetTank>, With<DisableRollback>)>,
    mut commands: Commands,
) {
    for (entity, predicted) in &fresh {
        info!("net: {entity} first physics tick complete — rollback enabled");
        let mut entity_commands = commands.entity(entity);
        entity_commands.remove::<DisableRollback>();
        if predicted {
            // The replicate-once value has seeded local PredictionHistory and survived the first
            // complete physics tick. From here rollback must restore only that local history.
            entity_commands.try_remove::<ConfirmedHistory<TrackGripElements>>();
        }
    }
}

/// Queue promotion when the independently replicated `Predicted` marker arrives late.
fn queue_predicted_promotion(
    add: On<Add, Predicted>,
    eligible: Query<(), (With<Remote>, With<NetTank>, With<Rig>)>,
    mut commands: Commands,
) {
    if !eligible.contains(add.entity) {
        return;
    }
    commands
        .entity(add.entity)
        .insert(PendingPredictedPromotion);
}

/// Promote only after the authoritative replicate-once element slab exists at the blueprint size.
fn upgrade_predicted_to_dynamic(
    candidates: Query<
        (Entity, &RigidBody, Option<&TrackGripElements>),
        (
            With<Remote>,
            With<NetTank>,
            With<Rig>,
            With<Predicted>,
            With<WeaponGate>,
            With<PendingPredictedPromotion>,
        ),
    >,
    source: TankSimSource,
    mut commands: Commands,
) {
    let Some(content) = source.get() else {
        return;
    };
    for (entity, body, grip_elements) in &candidates {
        if *body == RigidBody::Dynamic {
            commands
                .entity(entity)
                .remove::<PendingPredictedPromotion>();
            continue;
        }
        if !replica_role_ready(true, false, grip_elements, content.spec().track.link_count) {
            continue;
        }
        info!("net: {entity} predicted marker and exact grip seed ready — body goes Dynamic");
        commands
            .entity(entity)
            .insert((RigidBody::Dynamic, DisableRollback))
            .remove::<PendingPredictedPromotion>();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joining_predicted_role_cannot_attach_before_exact_element_seed() {
        let exact = TrackGripElements::for_links(97);
        let wrong_size = TrackGripElements::for_links(96);

        assert!(!replica_role_ready(true, false, None, 97));
        assert!(!replica_role_ready(true, false, Some(&wrong_size), 97));
        assert!(replica_role_ready(true, false, Some(&exact), 97));
        assert!(
            replica_role_ready(false, true, None, 97),
            "an interpolated remote deliberately has no private element field"
        );
        assert!(!replica_role_ready(false, false, Some(&exact), 97));
    }
}
