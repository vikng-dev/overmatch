//! The game's debris stream: admit peer poses before interpolation history sees them.

use bevy_replicon::prelude::RuleFns;
use bevy_replicon::shared::replication::registry::rule_fns::default_serialize;
use serde::{Deserialize, Serialize};

use super::{DebrisPose, Quat, Vec3};

/// The existing wire layout. Only the checked conversion produces a received `DebrisPose`.
#[derive(Serialize, Deserialize)]
pub(super) struct WirePose {
    position: Vec3,
    rotation: Quat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PoseRefusal {
    Encoding(postcard::Error),
    Position,
    Rotation,
}

impl std::fmt::Display for PoseRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encoding(error) => write!(f, "Server sent an unreadable debris pose: {error}"),
            Self::Position => f.write_str("Server sent a debris position that is not finite."),
            Self::Rotation => f.write_str(
                "Server sent an invalid debris rotation (expected a finite unit quaternion).",
            ),
        }
    }
}

impl std::error::Error for PoseRefusal {}

impl TryFrom<WirePose> for DebrisPose {
    type Error = PoseRefusal;

    fn try_from(wire: WirePose) -> Result<Self, Self::Error> {
        if !wire.position.is_finite() {
            return Err(PoseRefusal::Position);
        }
        // glam's slerp requires normalized inputs; use its own predicate at this boundary.
        if !wire.rotation.is_finite() || !wire.rotation.is_normalized() {
            return Err(PoseRefusal::Rotation);
        }
        Ok(Self {
            position: wire.position,
            rotation: wire.rotation,
        })
    }
}

pub(in crate::net) fn replication() -> RuleFns<DebrisPose> {
    RuleFns::new(default_serialize, |ctx, message| {
        let pose = bevy_replicon::postcard_utils::from_buf::<WirePose, _>(message)
            .map_err(PoseRefusal::Encoding)
            .and_then(DebrisPose::try_from);
        pose.map_err(|refusal| {
            // Replicon logs decoder errors but keeps the link. Retain the typed first cause for
            // `refuse_peer_pose`, which hands it to the client's visible match-close path.
            ctx.storage.global.get_or_init(|| refusal.clone());
            refusal.into()
        })
    })
}

#[cfg(test)]
mod tests;
