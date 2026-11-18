//! Admit complete servo poses against the tank data before interpolation sees a sample.

use bevy::prelude::*;
use bevy_replicon::prelude::{ClientSystems, ReplicationStorage, RuleFns};
use bevy_replicon::shared::replication::registry::rule_fns::default_serialize;

use super::matchmaking::ServerGone;
use super::protocol::{ServoAngles, ServoAnglesError, ServoAnglesWire};
use crate::tank::TankTemplate;

/// This composition's admitted tank rig. The template owns the slot count and ordering.
struct ServoSlots(usize);

pub(super) fn install(app: &mut App) {
    app.init_resource::<ReplicationStorage>()
        .add_message::<ServerGone>()
        .add_systems(PreUpdate, publish_slots.before(ClientSystems::Receive))
        .add_systems(PreUpdate, refuse_pose.after(ClientSystems::Receive));
}

fn publish_slots(template: Option<Res<TankTemplate>>, mut storage: ResMut<ReplicationStorage>) {
    // Startup without a template has no admitted rig and cannot connect to a match.
    if let Some(template) = template {
        storage
            .global
            .insert(ServoSlots(template.spec().servos.len()));
    }
}

pub(super) fn replication() -> RuleFns<ServoAngles> {
    RuleFns::new(default_serialize, |ctx, message| {
        let pose = bevy_replicon::postcard_utils::from_buf::<ServoAnglesWire, _>(message)
            .map_err(ServoAnglesError::Encoding)
            .and_then(ServoAngles::try_from)
            .and_then(|pose| {
                let slots = ctx
                    .storage
                    .global
                    .get::<ServoSlots>()
                    .ok_or(ServoAnglesError::MissingRig)?;
                pose.check_count(slots.0)?;
                Ok(pose)
            });
        pose.map_err(|refusal| {
            ctx.storage.global.get_or_init(|| refusal.clone());
            refusal.into()
        })
    })
}

fn refuse_pose(mut storage: ResMut<ReplicationStorage>, mut gone: MessageWriter<ServerGone>) {
    // No refusal is the normal state of a valid stream.
    if let Some(refusal) = storage.global.remove::<ServoAnglesError>() {
        warn!("client: refusing server servo pose: {refusal}");
        gone.write(ServerGone::Servo(refusal));
    }
}

#[cfg(test)]
mod tests;
