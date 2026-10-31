//! The Battle's netcode handshake gate. The authority publishes admission once per tick; both
//! handshake stages consult it. Refusals cross netcode as authenticated denial reasons.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::prelude::*;
use lightyear::connection::shared::{ConnectionRequestHandler, DeniedReason};
use lightyear::prelude::PeerId;

const BATTLE_CLOSED: &str = "overmatch:battle_late_join_closed";

/// A terminal admission refusal. The client shows this cause and stops retrying this server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JoinRefusal {
    BattleClosed,
    ServerFull,
    Rejected,
}

impl JoinRefusal {
    pub(super) fn from_netcode(reason: &DeniedReason) -> Self {
        match reason {
            DeniedReason::Custom(code) if code == BATTLE_CLOSED => Self::BattleClosed,
            DeniedReason::ServerFull => Self::ServerFull,
            // Other netcode refusals are terminal too; netcode logs their detailed cause.
            _ => Self::Rejected,
        }
    }

    pub(super) fn message(self) -> &'static str {
        match self {
            Self::BattleClosed => "This Battle has closed to late joins. Join another server.",
            Self::ServerFull => "This server is full. Join another server.",
            Self::Rejected => "The server refused this connection. Join another server.",
        }
    }
}

/// Shared with netcode's request handler. It starts open for the first fill-up; the Battle's
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
        (!self.is_open()).then(|| DeniedReason::Custom(BATTLE_CLOSED.into()))
    }
}
