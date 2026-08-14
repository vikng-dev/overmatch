//! Local simulation attachment for replicated tank roots.
//!
//! ADR-0014 exception: a replicated root receives simulation only after it has a wire pose;
//! children derive from root simulation. Every replica — the own hull included — rides the
//! interpolated server stream on a `Static` body.

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::prelude::*;
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use super::protocol::NetTank;
use crate::tank::{PendingTankAssets, TankSimSource, attach_replicated_tank_body};
use crate::track::sim::TankTransmission;

/// Attach simulation from `TankSimSource` only after a replicated root has a valid pose.
pub(crate) fn attach_replicated_rig(
    tanks: Query<
        Entity,
        (
            With<Remote>,
            With<NetTank>,
            // Wait until Lightyear declares the replica's role.
            With<Interpolated>,
            With<Position>,
            With<Rotation>,
            // The replicated current transmission snapshot must precede body attachment, so no
            // local consumer reads a freshly reconstructed JIP value.
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
    for entity in &tanks {
        info!("client: {entity} replicated tank gets local sim body");
        attach_replicated_tank_body(
            &mut commands,
            entity,
            content,
            assets.presentation(),
            (
                NetTank,
                // The local hierarchy requires `Transform`; Avian writes it from the wire pose.
                Transform::default(),
                RigidBody::Static,
            ),
        );
    }
}
