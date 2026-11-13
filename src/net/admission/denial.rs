//! The custom netcode denial this build's server sends and its client reads. A wire codec: the
//! wire-types pin hashes this file, so a changed code refuses skewed builds at the handshake.

use lightyear::connection::shared::DeniedReason;

const BATTLE_CLOSED: &str = "overmatch:battle_late_join_closed";

pub(super) fn battle_closed() -> DeniedReason {
    DeniedReason::Custom(BATTLE_CLOSED.into())
}

pub(super) fn is_battle_closed(reason: &DeniedReason) -> bool {
    matches!(reason, DeniedReason::Custom(code) if code == BATTLE_CLOSED)
}
