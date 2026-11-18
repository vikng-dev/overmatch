//! Admit complete servo poses against the tank data before interpolation sees a sample.

use bevy::prelude::*;
use bevy_replicon::prelude::{ClientSystems, ReplicationStorage, RuleFns};
use bevy_replicon::shared::replication::registry::rule_fns::default_serialize;

use super::matchmaking::ServerGone;
use super::protocol::{ServoAngles, ServoAnglesError, ServoAnglesWire};
use crate::tank::TankTemplate;

/// This composition's rig in the template's assembled slot order.
struct ServoSlots(Vec<Option<(f32, f32)>>);

impl ServoSlots {
    fn from_template(template: &TankTemplate) -> Self {
        Self(template.servo_travel_limits().collect())
    }
}

pub(super) fn install(app: &mut App) {
    if !app.is_plugin_added::<bevy_replicon::client::ClientPlugin>() {
        return;
    }
    app.init_resource::<ReplicationStorage>()
        .add_message::<ServerGone>()
        .add_systems(
            PreUpdate,
            publish_slots
                .before(ClientSystems::Receive)
                .run_if(resource_added::<TankTemplate>),
        )
        .add_systems(PreUpdate, refuse_pose.after(ClientSystems::Receive));
}

fn publish_slots(template: Res<TankTemplate>, mut storage: ResMut<ReplicationStorage>) {
    storage.global.insert(ServoSlots::from_template(&template));
}

/// Limited angles stay inside their authored stops; continuous angles are finite unwrapped f32
/// travel (`advance_servo` accumulates rather than wrapping at a full turn).
fn admit(
    bytes: &mut impl bevy_replicon::bytes::Buf,
    slots: Option<&ServoSlots>,
) -> Result<ServoAngles, ServoAnglesError> {
    let wire = bevy_replicon::postcard_utils::from_buf::<ServoAnglesWire, _>(bytes)
        .map_err(ServoAnglesError::Encoding)?;
    let pose = ServoAngles::try_from(wire)?;
    let slots = slots.ok_or(ServoAnglesError::MissingRig)?;
    if pose.0.len() != slots.0.len() {
        return Err(ServoAnglesError::Count {
            expected: slots.0.len(),
            received: pose.0.len(),
        });
    }
    for (slot, (&angle, limits)) in pose.0.iter().zip(&slots.0).enumerate() {
        if let Some((min, max)) = limits
            && !(*min..=*max).contains(&angle)
        {
            return Err(ServoAnglesError::OutOfTravel { slot });
        }
    }
    Ok(pose)
}

pub(super) fn replication() -> RuleFns<ServoAngles> {
    RuleFns::new(default_serialize, |ctx, message| {
        let pose = admit(message, ctx.storage.global.get::<ServoSlots>());
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
