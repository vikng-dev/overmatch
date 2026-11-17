//! Fixed twelve-byte codec; stale stamp decoding relies on Ledger's `replicate_diff` registration
//! setting Replicon's global `registry.needs_history()`. Without that registration, stale entity
//! mutations skip deserialization entirely. See the matching protocol registration comment.

use super::{ReceivedStamps, TickStamp};
use bevy_replicon::prelude::RuleFns;
use bevy_replicon::shared::replication::registry::rule_fns::default_serialize;
use serde::{Deserialize, Deserializer};

impl TickStamp {
    pub(in crate::net) fn new(
        frame_start_s: f64,
        departure_offset_s: f32,
    ) -> Result<Self, InvalidStamp> {
        if !frame_start_s.is_finite()
            || frame_start_s < 0.0
            || !departure_offset_s.is_finite()
            || departure_offset_s < 0.0
            || !(frame_start_s + f64::from(departure_offset_s)).is_finite()
        {
            return Err(InvalidStamp);
        }
        Ok(Self {
            frame_start_s,
            departure_offset_s,
        })
    }
}

impl<'de> Deserialize<'de> for TickStamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Fields {
            frame_start_s: f64,
            departure_offset_s: f32,
        }
        let fields = Fields::deserialize(deserializer)?;
        TickStamp::new(fields.frame_start_s, fields.departure_offset_s)
            .map_err(serde::de::Error::custom)
    }
}

pub(in crate::net) fn replication() -> RuleFns<TickStamp> {
    RuleFns::new(default_serialize, |ctx, message| {
        let stamp: TickStamp = bevy_replicon::postcard_utils::from_buf(message)?;
        ctx.storage
            .global
            .get_or_default::<ReceivedStamps>()
            .push(ctx.message_tick, stamp);
        Ok(stamp)
    })
}

#[derive(Debug)]
pub(in crate::net) struct InvalidStamp;
impl std::fmt::Display for InvalidStamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "server tick frame start and departure offset must be finite nonnegative seconds",
        )
    }
}
impl std::error::Error for InvalidStamp {}
