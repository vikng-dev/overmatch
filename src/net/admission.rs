//! The Battle's netcode handshake gate. The authority publishes admission once per tick; both
//! handshake stages consult it. Refusals cross netcode as authenticated denial reasons.

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::prelude::*;
use lightyear::connection::shared::{ConnectionRequestHandler, DeniedReason};
use lightyear::prelude::PeerId;

use super::compatibility::{Compatibility, Incompatibility};

mod denial;

/// A terminal admission refusal. The client shows this cause and stops retrying this server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum JoinRefusal {
    BattleClosed,
    ServerFull,
    /// The server runs another build: its record against the one this link's token states.
    Incompatible(Incompatibility),
    Rejected,
}

impl JoinRefusal {
    pub(super) fn from_netcode(reason: &DeniedReason, stated: &Compatibility) -> Self {
        match reason {
            reason if denial::is_battle_closed(reason) => Self::BattleClosed,
            DeniedReason::Custom(record) => match Compatibility::parse(record) {
                Ok(server) => match Incompatibility::between(stated, &server) {
                    Some(cause) => Self::Incompatible(cause),
                    None => {
                        warn!(
                            "client: the server refused this client as another build, stating \
                             this client's own record"
                        );
                        Self::Rejected
                    }
                },
                Err(error) => {
                    warn!(
                        ?error,
                        "client: the server refused with a reason this build cannot read"
                    );
                    Self::Rejected
                }
            },
            DeniedReason::ServerFull => Self::ServerFull,
            // Other netcode refusals are terminal too; netcode logs their detailed cause.
            _ => Self::Rejected,
        }
    }

    pub(super) fn message(&self) -> Cow<'static, str> {
        match self {
            Self::BattleClosed => {
                "This Battle has closed to late joins. Join another server.".into()
            }
            Self::ServerFull => "This server is full. Join another server.".into(),
            Self::Incompatible(cause) => cause.message().into(),
            Self::Rejected => "The server refused this connection. Join another server.".into(),
        }
    }
}

/// Shared with netcode's request handler. It starts open for the first player; the Battle's
/// authority is its sole writer. Hosting reads the same gate for its heartbeat.
#[derive(Resource, Clone, Debug)]
pub(super) struct JoinAdmission(Arc<AtomicBool>);

impl Default for JoinAdmission {
    fn default() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }
}

impl JoinAdmission {
    pub(super) fn is_open(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub(super) fn set_open(&self, open: bool) {
        self.0.store(open, Ordering::Relaxed);
    }
}

impl ConnectionRequestHandler for JoinAdmission {
    fn handle_request(&self, _client: PeerId) -> Option<DeniedReason> {
        (!self.is_open()).then(denial::battle_closed)
    }
}
