//! Fixed f64 stamp codec; decoded history is kept even when one frame drains many ticks.

use super::{ReceivedStamps, TickStamp};
use bevy_replicon::prelude::RuleFns;
use bevy_replicon::shared::replication::registry::rule_fns::default_serialize;

pub(in crate::net) fn replication() -> RuleFns<TickStamp> {
    RuleFns::new(default_serialize, |ctx, message| {
        let stamp: TickStamp = bevy_replicon::postcard_utils::from_buf(message)?;
        if !stamp.0.is_finite() || stamp.0 < 0.0 {
            return Err(InvalidStamp.into());
        }
        ctx.storage
            .global
            .get_or_default::<ReceivedStamps>()
            .0
            .push((ctx.message_tick, stamp));
        Ok(stamp)
    })
}

#[derive(Debug)]
struct InvalidStamp;
impl std::fmt::Display for InvalidStamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("server tick stamp must be finite nonnegative real seconds")
    }
}
impl std::error::Error for InvalidStamp {}
