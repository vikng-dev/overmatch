//! Local simulation attachment for replicated tank roots.
//!
//! ADR-0014 exception: a replicated root receives simulation only after it has a wire pose;
//! children derive from root simulation. Every replica — the own hull included — rides the
//! interpolated server stream on a `Static` body.

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::prelude::*;
use lightyear::interpolation::plugin::InterpolationSystems;
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use super::protocol::NetTank;
use crate::tank::{PendingTankAssets, TankSimSource, attach_replicated_tank_body};
use crate::track::sim::EngineRpm;

/// Attach simulation only after interpolation has established this frame's visible component
/// set. An init snapshot may contain a pose whose first sample is still ahead of the cursor;
/// attaching before presence preparation would give that not-yet-visible root live consumers.
pub(crate) fn attach_replicated_rig() -> ScheduleConfigs<ScheduleSystem> {
    attach_visible_rigs
        .after(InterpolationSystems::All)
        .into_configs()
}

fn attach_visible_rigs(
    tanks: Query<
        Entity,
        (
            With<Remote>,
            With<NetTank>,
            // Wait until Lightyear declares the replica's role.
            With<Interpolated>,
            With<Position>,
            With<Rotation>,
            // The replicated public crank speed must precede body attachment, so the engine sound
            // the body brings is born at the authority's crank, never a reconstructed JIP value.
            // It is the one transmission fact every client receives: `TankTransmission` reaches
            // only the owner, in the same init snapshot.
            With<EngineRpm>,
            Without<RigidBody>,
        ),
    >,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    mut commands: Commands,
) {
    if tanks.is_empty() {
        return;
    }
    let content = source.get();
    for entity in &tanks {
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
